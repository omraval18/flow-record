# fr shell integration (bash 3.2+). Add to ~/.bashrc:
#   eval "$(fr init bash)"
#
# bash has no preexec hook, so commands are read back from history after they finish
# (PROMPT_COMMAND). Commands bash keeps out of history (HISTCONTROL=ignorespace, a
# disabled history) are therefore not recorded either.

FR_SESSION="" FR_OLD_PS1="" FR_RESUME="" FR_RESUME_STEP=""
__fr_depth=0 __fr_dir=$PWD __fr_last="" __fr_ready="" __fr_typed=""

__fr_start() {
  if [[ -n $FR_SESSION ]]; then echo "error: already recording — run \`fr\` or \`fr stop\` to finish" >&2; return 1; fi
  FR_SESSION=$(command fr record bash) || { FR_SESSION=""; return 1; }
  FR_OLD_PS1=$PS1
  PS1="\[\e[31m\]● rec\[\e[0m\] $PS1"
  echo "  ● Recording. Run your commands, then \`fr\` again to save." >&2
}

__fr_stop() {
  if [[ -z $FR_SESSION ]]; then echo "error: not recording — run \`fr\` to start" >&2; return 1; fi
  local f=$FR_SESSION
  FR_SESSION=""
  PS1=$FR_OLD_PS1
  command fr stop "$f"
}

# Replayed steps run in *this* shell so cd/export/source persist.
__fr_run() {
  local __fr_s __fr_i __fr_rc
  local __fr_depth=$(( __fr_depth + 1 ))
  __fr_s=$(command fr _exec bash "$__fr_depth" "$@") || return $?
  [[ -n $__fr_s ]] || return 0
  # The steps about to be added to history would hide the command that was typed.
  (( __fr_depth == 1 )) && __fr_typed=$(HISTTIMEFORMAT='' builtin history 1)
  eval "$__fr_s"
}

__fr_resume() {
  if [[ -z $FR_RESUME ]]; then echo "error: nothing to resume — the last workflow did not fail" >&2; return 1; fi
  eval "__fr_run --from $FR_RESUME_STEP $FR_RESUME"
}

# Replayed steps go into history, so ↑ and Ctrl-R find them.
__fr_remember() { builtin history -s -- "$1"; return 0; }

# `fr shim next` makes `next` mean `fr next`.
__fr_shim() { eval "function $1 { fr $1 \"\$@\"; }"; }

fr() {
  case "${1-}" in
    "")            if [[ -n $FR_SESSION ]]; then __fr_stop; else __fr_start; fi ;;
    record|start)  __fr_start ;;
    stop)          __fr_stop ;;
    status)        if [[ -n $FR_SESSION ]]; then echo "recording"; else echo "not recording"; fi ;;
    run)           shift; __fr_run "$@" ;;
    resume)        __fr_resume ;;
    save)          shift; command fr _save bash $$ "$@" ;;
    suggest)       command fr _suggest bash ;;
    shim)          command fr "$@" && { local n; shift; for n in "$@"; do __fr_shim "$n"; done; } ;;
    unshim)        command fr "$@" && { local n; shift; for n in "$@"; do unset -f "$n"; done; } ;;
    list|ls|show|edit|rm|delete|remove|export|init|version|help|-h|--help|-V|--version) command fr "$@" ;;
    *)             __fr_run "$@" ;;
  esac
}

# After each command: read it back from history and log it with its status. Only
# builtins run here unless a recording is active, so this costs one fork (history 1).
# Set FR_NO_HISTORY=1 to stop logging.
__fr_precmd() {
  local s=$? h line c d
  h=$(HISTTIMEFORMAT='' builtin history 1)
  if [[ -n $__fr_typed ]]; then
    line=$__fr_typed __fr_typed=""
  elif [[ -n $__fr_ready && $h != "$__fr_last" ]]; then
    line=$h
  fi
  __fr_last=$h __fr_ready=1
  if [[ -n $line ]]; then
    # "  123  command" (or "  123* command" when edited)
    c=${line#"${line%%[![:space:]]*}"}
    c=${c#"${c%%[![:digit:]]*}"}
    c=${c#\*}; c=${c# }; c=${c# }
    if [[ -n $c ]]; then
      [[ -n $FR_SESSION ]] && command fr _ev "$FR_SESSION" "$s" "$c"
      if [[ -z ${FR_NO_HISTORY-} ]]; then
        c=${c//\\/\\\\}; c=${c//$'\n'/\\n}; c=${c//$'\t'/\\t}
        d=${__fr_dir//\\/\\\\}; d=${d//$'\n'/\\n}; d=${d//$'\t'/\\t}
        printf '%s\t%s\t%s\t%s\t%s\n' "${EPOCHSECONDS-}" "$$" "$s" "$d" "$c" >> "$__fr_log" 2>/dev/null
      fi
    fi
  fi
  __fr_dir=$PWD
  return $s
}
case ";${PROMPT_COMMAND-};" in
  *";__fr_precmd;"*) ;;
  *) PROMPT_COMMAND="__fr_precmd${PROMPT_COMMAND:+;$PROMPT_COMMAND}" ;; # first, so $? is intact
esac
[[ -d ${__fr_log%/*} ]] || mkdir -p "${__fr_log%/*}" 2>/dev/null

# Completion: workflows ranked for this folder, then commands; choices for arguments.
_fr() {
  local cur=${COMP_WORDS[COMP_CWORD]} IFS=$'\n' line words=() sub=${COMP_WORDS[1]-}
  COMPREPLY=()
  if (( COMP_CWORD == 1 )); then
    for line in $(command fr _complete 2>/dev/null); do words+=("${line%%:*}"); done
    words+=(record stop status save suggest run resume list show edit rm export shim unshim)
  elif (( COMP_CWORD == 2 )) && case $sub in run|show|edit|rm|delete|remove|export|shim) true ;; *) false ;; esac; then
    for line in $(command fr _complete 2>/dev/null); do words+=("${line%%:*}"); done
  elif [[ $sub == unshim ]]; then
    words=($(command fr shim 2>/dev/null))
  else
    local name=$sub index=$(( COMP_CWORD - 2 ))
    if [[ $sub == run ]]; then name=${COMP_WORDS[2]-} index=$(( COMP_CWORD - 3 )); fi
    words=($(command fr _complete "$name" "$index" 2>/dev/null))
    words=("${words[@]:1}") # the first line is a hint, the rest are choices
  fi
  (( ${#words[@]} )) && COMPREPLY=($(compgen -W "${words[*]}" -- "$cur"))
  return 0
}
complete -F _fr fr
