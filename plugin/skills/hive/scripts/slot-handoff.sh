#!/bin/bash
set +x +a
export -n SHELLOPTS
unset credential nonce request step_request producer_request

# Private entry point used by the installed guide.
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

# Keep this Bash process as the parent of every lock-bearing checkpoint.
take_slot() {
    local binary=$1 repo=$2 pool=$3 slot=$4 expect=$5 commit=$6
    local reserve_op=$7 begin_op=$8 ready_op=$9 mode=${10}
    local id_pattern='^[0-9a-f]{32}$' key_pattern='^[0-9a-f]{64}$'
    local oid_pattern='^([0-9a-f]{40}|[0-9a-f]{64})$' count_pattern='^(0|[1-9][0-9]*)$'
    local hive_root=${HIVE_HOME:-$HOME/.hive} credential nonce extra request step_request
    local lock generation permit attempt checkout witnessed_commit prefix suffix digit
    local scratch
    local -a scope

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
            IFS= read -r -d '' witnessed_commit || return 1
        extra=
        if IFS= read -r -d '' extra || [[ -n $extra ]]; then return 1; fi
    } < "$scratch/permit"
    [[ $permit == permit_issued && $attempt =~ $count_pattern && $attempt != 0 &&
        $checkout == /* && $witnessed_commit == "$commit" ]] || return 1

    printf -v step_request '{"auth":%s,"step":"checkout"}' "$request"
    "$binary" slot step-check "${scope[@]}" --generation "$generation" --attempt "$attempt" \
        --lock-fd 9 --json <<< "$step_request" > "$scratch/result" || { /bin/cat "$scratch/result"; return 1; }
    /usr/bin/env -i HOME="$HOME" PATH=/usr/bin:/bin GIT_NO_LAZY_FETCH=1 GIT_ALLOW_PROTOCOL= \
        /usr/bin/git --no-replace-objects -c core.fsmonitor=false -c core.hooksPath=/dev/null \
        -c submodule.recurse=false -C "$repo" worktree add --detach -- "$checkout" "$commit" \
        > /dev/null || return 1
    printf -v step_request '{"auth":%s,"step":"checkout_done"}' "$request"
    "$binary" slot step-check "${scope[@]}" --generation "$generation" --attempt "$attempt" \
        --lock-fd 9 --json <<< "$step_request" > "$scratch/result" || { /bin/cat "$scratch/result"; return 1; }
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

produce_slot() {
    local binary=$1 repo=$2 pool=$3 slot=$4 generation=$5 operation=$6 invocation=$7
    local id_pattern='^[0-9a-f]{32}$' key_pattern='^[0-9a-f]{64}$'
    local count_pattern='^[1-9][0-9]{0,2}$' invocation_pattern='^[a-zA-Z0-9_.-]{1,64}$'
    local hive_root=${HIVE_HOME:-$HOME/.hive} credential nonce producer_request extra
    local scratch field cwd total i stopped writer cleanup stop_cleanup
    local -a scope assignments argv
    check_slot_version "$binary" || return 1
    [[ $repo == /* && $hive_root == /* && ( $slot == 1 || $slot == 2 ) &&
        $pool =~ $id_pattern && $operation =~ $id_pattern &&
        $generation =~ ^[1-9][0-9]{0,19}$ && $invocation =~ $invocation_pattern ]] || return 1
    IFS= read -r credential && [[ $credential =~ $key_pattern ]] || return 1
    extra=
    if IFS= read -r extra || [[ -n $extra ]]; then return 1; fi
    umask 077
    scratch=$(/usr/bin/mktemp -d "${TMPDIR:-/private/tmp}/hive-slot-producer.XXXXXXXX") || return 1
    printf -v cleanup '/bin/rm -f -- %q %q %q; /bin/rmdir -- %q' "$scratch/nonce" "$scratch/permit" "$scratch/result" "$scratch"
    trap "$cleanup" EXIT
    : > "$scratch/nonce" && : > "$scratch/permit" || return 1
    "$binary" slot key --out-fd 8 --json 8<> "$scratch/nonce" > "$scratch/result" || return 1
    IFS= read -r nonce < "$scratch/nonce" && [[ $nonce =~ $key_pattern ]] || return 1
    scope=(--repo "$repo" --pool "$pool" --slot "$slot" --generation "$generation" --op "$operation" --lock-fd 9)
    exec 9< "$hive_root/slot-locks/$pool/slot-$slot.lock" || return 1
    printf -v producer_request '{"action":"guided_start","credential":"%s","nonce":"%s","invocation":"%s"}' "$credential" "$nonce" "$invocation"
    "$binary" slot producer "${scope[@]}" --control-fd 8 --json <<< "$producer_request"         8<> "$scratch/permit" > "$scratch/result" || { /bin/cat "$scratch/result"; return 1; }
    assignments=() argv=()
    {
        IFS= read -r -d '' field && [[ $field == producer_permit ]] &&
            IFS= read -r -d '' cwd && [[ $cwd == /* ]] &&
            IFS= read -r -d '' total && [[ $total =~ $count_pattern && $total -le 256 ]] || return 1
        for (( i=0; i<total; i++ )); do
            IFS= read -r -d '' field && [[ $field =~ ^[a-zA-Z_][a-zA-Z0-9_]*= ]] || return 1
            assignments+=("$field")
        done
        IFS= read -r -d '' total && [[ $total =~ $count_pattern && $total -le 65 ]] || return 1
        for (( i=0; i<total; i++ )); do
            IFS= read -r -d '' field || return 1
            argv+=("$field")
        done
        extra=
        if IFS= read -r -d '' extra || [[ -n $extra ]]; then return 1; fi
    } < "$scratch/permit"
    [[ ${argv[0]} == /* ]] || return 1
    cd -P -- "$cwd" || return 1
    set -m
    /usr/bin/env -i /bin/bash --noprofile --norc -c         'builtin kill -STOP "$$"; builtin exec -c /usr/bin/env -i "$@"'         producer-gate "${assignments[@]}" "${argv[@]}" </dev/null >&2 &
    writer=$!
    printf -v stop_cleanup 'builtin kill -KILL %q 2>/dev/null; wait %q 2>/dev/null; %s' "$writer" "$writer" "$cleanup"
    trap "$stop_cleanup" EXIT
    for (( i=0; i<250; i++ )); do
        stopped=$(/bin/ps -p "$writer" -o state= 9<&-) || return 1
        [[ $stopped =~ ^[[:space:]]*T ]] && break
        /bin/sleep 0.02 9<&- || return 1
    done
    [[ $stopped =~ ^[[:space:]]*T ]] || return 1
    printf -v producer_request '{"action":"guided_watch","credential":"%s","nonce":"%s","invocation":"%s","child_pid":%s}' "$credential" "$nonce" "$invocation" "$writer"
    "$binary" slot producer "${scope[@]}" --json <<< "$producer_request" > "$scratch/result" || {
        /bin/cat "$scratch/result"
        return 1
    }
    wait "$writer" || return 1
    trap "$cleanup" EXIT
    /bin/cat "$scratch/result"
}

if [[ $# == 2 && $1 == check-version ]]; then
    if check_slot_version "$2"; then
        exit 0
    fi
    printf '%s\n' 'Slot use requires the exact cargo-hive version bundled with this guide. Update the plugin or reinstall the matching binary.' >&2
    exit 1
fi

if [[ $# == 10 && ( $1 == take || $1 == take-seeded ) ]]; then
    set +x
    mode=$1
    shift
    take_slot "$@" "$mode" && exit 0
    printf '%s\n' 'Slot preparation could not be confirmed. Inspect the slot before retrying.' >&2
    exit 1
fi

if [[ $# == 8 && $1 == produce ]]; then
    shift
    produce_slot "$@" && exit 0
    printf '%s\n' 'Slot producer could not be confirmed. Preserve its pending or held evidence.' >&2
    exit 1
fi

printf '%s\n' 'Expected check-version or a private slot request.' >&2
exit 2
