#!/bin/bash

# Private entry point used by the installed guide.
set +x +a
unset -v credential nonce request step_request extra
export -n SHELLOPTS
check_slot_version() {
    local binary=$1 metadata version actual
    local version_pattern='^  hive-version: "([0-9]+\.[0-9]+\.[0-9]+)"$'
    local line in_metadata=0 closed=0
    metadata=$(cd -P -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P) || return 1
    version=
    {
        IFS= read -r line && [[ $line == --- ]] || return 1
        while IFS= read -r line; do
            case $line in
                ---) closed=1; break ;;
                metadata:) in_metadata=1 ;;
                '  hive-version:'*)
                    [[ $in_metadata == 1 && -z $version && $line =~ $version_pattern ]] || return 1
                    version=${BASH_REMATCH[1]}
                    ;;
                [!\ ]*) in_metadata=0 ;;
            esac
        done
    } < "$metadata/SKILL.md"
    [[ $closed == 1 && -n $version && $binary == /* && -f $binary && -x $binary ]] || return 1

    # Keep the trailing newline so extra output cannot be mistaken for the version.
    actual=$("$binary" --version && printf '.') || return 1
    [[ $actual == "cargo-hive $version"$'\n.' ]]
}

git_for_slot() {
    /usr/bin/env -i PATH=/usr/bin:/bin GIT_NO_LAZY_FETCH=1 GIT_ALLOW_PROTOCOL= \
        GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null \
        /usr/bin/git --no-replace-objects -c core.fsmonitor=false -c core.hooksPath=/dev/null \
        -c submodule.recurse=false -c maintenance.auto=false -c gc.auto=0 "$@"
}

# Keep this Bash process as the parent of every lock-bearing checkpoint.
take_slot() {
    local binary=$1 repo=$2 pool=$3 slot=$4 expect=$5 commit=$6
    local reserve_op=$7 begin_op=$8 ready_op=$9 mode=${10}
    local id_pattern='^[0-9a-f]{32}$' key_pattern='^[0-9a-f]{64}$'
    local oid_pattern='^([0-9a-f]{40}|[0-9a-f]{64})$' count_pattern='^(0|[1-9][0-9]*)$'
    local hive_root=${HIVE_HOME:-$HOME/.hive} credential nonce extra request step_request
    local lock generation permit attempt checkout witnessed_commit prefix suffix digit
    local scratch
    local prior anchor index anchor_path parent
    local -a scope anchors

    check_slot_version "$binary" || return 1
    [[ $repo == /* && ( $slot == 1 || $slot == 2 ) && $expect =~ $count_pattern && ${#expect} -le 20 &&
        $pool =~ $id_pattern && $commit =~ $oid_pattern && $reserve_op =~ $id_pattern &&
        $begin_op =~ $id_pattern && $ready_op =~ $id_pattern && $hive_root == /* &&
        $reserve_op != "$begin_op" && $reserve_op != "$ready_op" && $begin_op != "$ready_op" ]] || return 1
    IFS= read -r credential && [[ $credential =~ $key_pattern ]] || return 1
    extra=
    if IFS= read -r extra || [[ -n $extra ]]; then return 1; fi

    umask 077
    scratch=$(/usr/bin/mktemp -d "${TMPDIR:-/private/tmp}/hive-slot-guide.XXXXXXXX") || return 1
    local cleanup
    printf -v cleanup '/bin/rm -f -- %q %q %q; /bin/rmdir -- %q' \
        "$scratch/nonce" "$scratch/permit" "$scratch/result" "$scratch"
    trap "$cleanup" EXIT
    : > "$scratch/nonce" && : > "$scratch/permit" || return 1
    "$binary" slot key --out-fd 8 --json 8<> "$scratch/nonce" > "$scratch/result" || { /bin/cat "$scratch/result"; return 1; }
    IFS= read -r nonce < "$scratch/nonce" && [[ $nonce =~ $key_pattern ]] || return 1
    printf -v request '{"credential":"%s","nonce":"%s"}' "$credential" "$nonce"
    scope=(--repo "$repo" --pool "$pool" --slot "$slot")
    # The slot generation is unsigned. Increment decimal digits without shell overflow.
    prefix=$expect suffix=
    while [[ $prefix == *9 ]]; do prefix=${prefix%9}; suffix=0$suffix; done
    if [[ -z $prefix ]]; then
        generation=1$suffix
    else
        digit=${prefix:${#prefix}-1}
        generation=${prefix%?}$((digit + 1))$suffix
    fi
    "$binary" slot reserve "${scope[@]}" --expect "$expect" --commit "$commit" \
        --op "$reserve_op" --json <<< "$credential" > "$scratch/result" || { /bin/cat "$scratch/result"; return 1; }
    "$binary" slot ack "${scope[@]}" --generation "$generation" --op "$reserve_op" \
        --json <<< "$credential" > "$scratch/result" || { /bin/cat "$scratch/result"; return 1; }
    lock="$hive_root/slot-locks/$pool/slot-$slot.lock"
    exec 9< "$lock" || return 1
    "$binary" slot begin "${scope[@]}" --generation "$generation" --op "$begin_op" --lock-fd 9 \
        --control-fd 8 --json <<< "$request" 8<> "$scratch/permit" > "$scratch/result" || { /bin/cat "$scratch/result"; return 1; }
    {
        IFS= read -r -d '' permit && IFS= read -r -d '' attempt && IFS= read -r -d '' checkout &&
            IFS= read -r -d '' witnessed_commit && IFS= read -r -d '' prior || return 1
        anchors=()
        while IFS= read -r -d '' anchor; do
            [[ -n $anchor && $anchor != /* && ${#anchors[@]} -lt 256 ]] || return 1
            anchors[${#anchors[@]}]=$anchor
        done
        [[ -z $anchor ]] || return 1
    } < "$scratch/permit"
    [[ $permit == permit_issued && $attempt =~ $count_pattern && $attempt != 0 &&
        $checkout == /* && $witnessed_commit == "$commit" ]] || return 1

    printf -v step_request '{"auth":%s,"step":"checkout"}' "$request"
    "$binary" slot step-check "${scope[@]}" --generation "$generation" --attempt "$attempt" \
        --lock-fd 9 --json <<< "$step_request" > "$scratch/result" || { /bin/cat "$scratch/result"; return 1; }
    if [[ -z $prior ]]; then
        git_for_slot -C "$repo" worktree add --detach -- "$checkout" "$commit" > /dev/null || return 1
    elif [[ $prior != "$commit" ]]; then
        git_for_slot -C "$checkout" switch --detach -- "$commit" > /dev/null || return 1
    fi
    printf -v step_request '{"auth":%s,"step":"checkout_done"}' "$request"
    "$binary" slot step-check "${scope[@]}" --generation "$generation" --attempt "$attempt" \
        --lock-fd 9 --json <<< "$step_request" > "$scratch/result" || { /bin/cat "$scratch/result"; return 1; }
    if [[ ${#anchors[@]} -gt 0 ]]; then
        printf -v step_request '{"auth":%s,"step":"anchors"}' "$request"
        "$binary" slot step-check "${scope[@]}" --generation "$generation" --attempt "$attempt" \
            --lock-fd 9 --json <<< "$step_request" > "$scratch/result" || { /bin/cat "$scratch/result"; return 1; }
        for index in "${!anchors[@]}"; do
            anchor_path="$checkout/${anchors[$index]}"
            parent=${anchor_path%/*}
            [[ -f $anchor_path && ! -L $anchor_path && $(cd -P -- "$parent" && pwd -P) == "$parent" ]] || return 1
            exec 8< "$anchor_path" || return 1
            printf -v step_request '{"auth":%s,"step":{"anchor_descriptor":{"index":%s,"fd":8}}}' "$request" "$index"
            "$binary" slot step-check "${scope[@]}" --generation "$generation" --attempt "$attempt" \
                --lock-fd 9 --json <<< "$step_request" > "$scratch/result" || { /bin/cat "$scratch/result"; return 1; }
            /usr/bin/touch -m -c -- /dev/fd/8 || return 1
            exec 8<&-
        done
        printf -v step_request '{"auth":%s,"step":"anchors_done"}' "$request"
        "$binary" slot step-check "${scope[@]}" --generation "$generation" --attempt "$attempt" \
            --lock-fd 9 --json <<< "$step_request" > "$scratch/result" || { /bin/cat "$scratch/result"; return 1; }
    fi
    if [[ $mode == take-seeded ]]; then
        printf -v step_request '{"auth":%s,"step":"seed"}' "$request"
        "$binary" slot step-check "${scope[@]}" --generation "$generation" --attempt "$attempt" \
            --lock-fd 9 --json <<< "$step_request" > "$scratch/result" || { /bin/cat "$scratch/result"; return 1; }
    fi
    "$binary" slot ready "${scope[@]}" --generation "$generation" --attempt "$attempt" \
        --op "$ready_op" --lock-fd 9 --json <<< "$request" > "$scratch/result" || {
        /bin/cat "$scratch/result"
        return 1
    }
    /bin/cat "$scratch/result" || return 1
}

if [[ $# == 2 && $1 == check-version ]]; then
    if check_slot_version "$2"; then
        exit 0
    fi
    printf '%s\n' 'Slot use requires the exact cargo-hive version bundled with this guide. Update the plugin or reinstall the matching binary.' >&2
    exit 1
fi

if [[ $# == 10 && ( $1 == take || $1 == take-seeded ) ]]; then
    mode=$1
    shift
    take_slot "$@" "$mode" && exit 0
    printf '%s\n' 'Slot preparation could not be confirmed. Inspect the slot before retrying.' >&2
    exit 1
fi

printf '%s\n' 'Expected check-version or a private take request.' >&2
exit 2
