job() { "$JOB_BINARY" "$@"; }
emit() { local item; for item in "$@"; do [[ $item == ${PREFIX}* ]] && print -r -- "$item"; done; }
compadd() { while [[ $1 != -- && $# -gt 0 ]]; do shift; done; shift; emit "$@"; }
_describe() { local -a items; items=("${(@P)4}"); emit "${(@)items%%:*}"; }
_files() { print -r -- "<files $*>"; }
_command_names() { print -r -- "<commands>"; }
_normal() { print -r -- "<normal $CURRENT ${words[1]}>"; }
source "$1"
words=("${(@Q)${(z)2}}")
CURRENT=${#words}
PREFIX=$words[CURRENT]
_job
exit 0
