#!/usr/bin/env bash

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

if [[ $# == 2 && $1 == check-version ]]; then
    if check_slot_version "$2"; then
        exit 0
    fi
    printf '%s\n' 'Slot use requires the exact cargo-hive version bundled with this guide. Update the plugin or reinstall the matching binary.' >&2
    exit 1
fi

printf '%s\n' 'Expected check-version and an absolute cargo-hive binary path.' >&2
exit 2
