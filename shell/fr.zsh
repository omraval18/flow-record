# fr shell integration (zsh). Add to ~/.zshrc, after compinit:
#   eval "$(fr init zsh)"

typeset -g FR_SESSION="" FR_CMD="" FR_OLD_RPROMPT=""

__fr_start() {
  if [[ -n $FR_SESSION ]]; then print -u2 "error: already recording — run \`fr\` or \`fr stop\` to finish"; return 1; fi
  FR_SESSION=$(command fr record) || { FR_SESSION=""; return 1; }
  FR_OLD_RPROMPT=$RPROMPT
  RPROMPT="%F{red}● rec%f${RPROMPT:+ $RPROMPT}"
  print -u2 "  ● Recording. Run your commands, then \`fr\` again to save."
}

__fr_stop() {
  if [[ -z $FR_SESSION ]]; then print -u2 "error: not recording — run \`fr\` to start"; return 1; fi
  local f=$FR_SESSION
  FR_SESSION="" FR_CMD=""
  RPROMPT=$FR_OLD_RPROMPT
  command fr stop "$f"
}

# Replayed steps run in *this* shell so cd/export/source persist.
__fr_run() {
  local __fr_s __fr_i __fr_rc
  __fr_s=$(command fr _run "$@") || return $?
  [[ -n $__fr_s ]] && eval "$__fr_s"
}

fr() {
  case "${1-}" in
    "")            if [[ -n $FR_SESSION ]]; then __fr_stop; else __fr_start; fi ;;
    record|start)  __fr_start ;;
    stop)          __fr_stop ;;
    status)        if [[ -n $FR_SESSION ]]; then echo "recording"; else echo "not recording"; fi ;;
    run)           shift; __fr_run "$@" ;;
    list|ls|show|edit|rm|delete|remove|export|init|version|help|-h|--help|-V|--version) command fr "$@" ;;
    *)             __fr_run "$@" ;;
  esac
}

# Recording hooks: capture the command as typed (preexec) and its status (precmd).
__fr_preexec() { [[ -n $FR_SESSION ]] && FR_CMD=$1; return 0 }
__fr_precmd() {
  local s=$?
  [[ -n $FR_SESSION && -n $FR_CMD ]] && command fr _ev "$FR_SESSION" "$s" "$FR_CMD"
  FR_CMD=""
  return $s
}
autoload -Uz add-zsh-hook
add-zsh-hook preexec __fr_preexec
precmd_functions=(__fr_precmd ${precmd_functions:#__fr_precmd}) # first, so $? is intact

# Completion: `fr <TAB>` lists workflows ranked for this folder (project first, then recent).
_fr() {
  local -a w
  local sub=$words[2]
  if (( CURRENT == 2 )); then
    local -a c
    w=(${(f)"$(command fr _complete 2>/dev/null)"})
    c=(
      'record:start recording'
      'stop:stop recording and save'
      'status:recording status'
      'run:run a workflow'
      'list:list workflows'
      'show:print a workflow'
      'edit:change name, scope, parameters, steps'
      'rm:delete a workflow'
      'export:export as a bash script'
    )
    (( ${#w} )) && _describe -V -t workflows 'workflow' w
    _describe -t commands 'command' c
  elif [[ $sub == (run|show|edit|rm|delete|remove|export) && CURRENT == 3 ]]; then
    w=(${(f)"$(command fr _complete 2>/dev/null)"})
    (( ${#w} )) && _describe -V -t workflows 'workflow' w
  elif [[ $sub == run ]]; then
    local h; h=$(command fr _complete "$words[3]" $(( CURRENT - 4 )) 2>/dev/null)
    [[ -n $h ]] && _message -r "$h"
  elif [[ $sub != (list|ls|show|edit|rm|delete|remove|export|init|record|start|stop|status) ]]; then
    local h; h=$(command fr _complete "$sub" $(( CURRENT - 3 )) 2>/dev/null)
    [[ -n $h ]] && _message -r "$h"
  fi
}
if (( $+functions[compdef] )); then
  compdef _fr fr
  zstyle ':completion:*:*:fr:*' menu select
fi
