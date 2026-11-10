# fr shell integration (zsh). Add to ~/.zshrc, after compinit:
#   eval "$(fr init zsh)"

typeset -g FR_SESSION="" FR_CMD="" FR_DIR="" FR_OLD_RPROMPT="" FR_RESUME="" FR_RESUME_STEP=""
typeset -gi __fr_depth=0
zmodload -F zsh/datetime p:EPOCHSECONDS 2>/dev/null

__fr_start() {
  if [[ -n $FR_SESSION ]]; then print -u2 "error: already recording — run \`fr\` or \`fr stop\` to finish"; return 1; fi
  FR_SESSION=$(command fr record zsh) || { FR_SESSION=""; return 1; }
  FR_OLD_RPROMPT=$RPROMPT
  RPROMPT="%F{red}● rec%f${RPROMPT:+ $RPROMPT}"
  print -u2 "  ● Recording. Run your commands, then \`fr\` again to save."
}

__fr_stop() {
  if [[ -z $FR_SESSION ]]; then print -u2 "error: not recording — run \`fr\` to start"; return 1; fi
  local f=$FR_SESSION
  FR_SESSION=""
  RPROMPT=$FR_OLD_RPROMPT
  command fr stop "$f"
}

# Replayed steps run in *this* shell so cd/export/source persist.
__fr_run() {
  local __fr_s __fr_i __fr_rc
  local -i __fr_depth=$(( __fr_depth + 1 ))
  __fr_s=$(command fr _exec zsh $__fr_depth "$@") || return $?
  [[ -n $__fr_s ]] && eval "$__fr_s"
}

__fr_resume() {
  if [[ -z $FR_RESUME ]]; then print -u2 "error: nothing to resume — the last workflow did not fail"; return 1; fi
  eval "__fr_run --from $FR_RESUME_STEP $FR_RESUME"
}

# Replayed steps go into history, so ↑ and Ctrl-R find them.
__fr_remember() { print -rs -- "$1"; return 0 }

# `fr shim next` makes `next` mean `fr next`.
__fr_shim() { eval "function $1 { fr $1 \"\$@\" }" }

fr() {
  case "${1-}" in
    "")            if [[ -n $FR_SESSION ]]; then __fr_stop; else __fr_start; fi ;;
    record|start)  __fr_start ;;
    stop)          __fr_stop ;;
    status)        if [[ -n $FR_SESSION ]]; then echo "recording"; else echo "not recording"; fi ;;
    run)           shift; __fr_run "$@" ;;
    resume)        __fr_resume ;;
    save)          shift; command fr _save zsh $$ "$@" ;;
    suggest)       command fr _suggest zsh ;;
    shim)          command fr "$@" && { local n; for n in "${@:2}"; do __fr_shim $n; done } ;;
    unshim)        command fr "$@" && { local n; for n in "${@:2}"; do unfunction $n 2>/dev/null; done; return 0 } ;;
    list|ls|show|edit|rm|delete|remove|export|init|version|help|-h|--help|-V|--version) command fr "$@" ;;
    *)             __fr_run "$@" ;;
  esac
}

# Hooks: note the command as typed (preexec), then log it with its status (precmd).
# Logging uses only builtins, so it costs no process. Set FR_NO_HISTORY=1 to stop it.
__fr_preexec() { FR_CMD=$1 FR_DIR=$PWD; return 0 }
__fr_precmd() {
  local s=$?
  if [[ -n $FR_CMD ]]; then
    [[ -n $FR_SESSION ]] && command fr _ev "$FR_SESSION" "$s" "$FR_CMD"
    if [[ -z ${FR_NO_HISTORY-} ]]; then
      local c=${FR_CMD//\\/\\\\} d=${FR_DIR//\\/\\\\}
      c=${c//$'\n'/\\n}; c=${c//$'\t'/\\t}; d=${d//$'\n'/\\n}; d=${d//$'\t'/\\t}
      print -r -- "${EPOCHSECONDS-}"$'\t'"$$"$'\t'"$s"$'\t'"$d"$'\t'"$c" >>| $__fr_log 2>/dev/null
    fi
  fi
  FR_CMD=""
  return $s
}
autoload -Uz add-zsh-hook
add-zsh-hook preexec __fr_preexec
precmd_functions=(__fr_precmd ${precmd_functions:#__fr_precmd}) # first, so $? is intact
[[ -d ${__fr_log:h} ]] || mkdir -p ${__fr_log:h} 2>/dev/null

# Completion: `fr <TAB>` lists workflows ranked for this folder (project first, then recent).
__fr_complete_arg() {
  local -a lines; lines=(${(f)"$(command fr _complete "$1" $2 2>/dev/null)"})
  (( ${#lines} )) || return
  if (( ${#lines} > 1 )); then
    compadd -X "${lines[1]}" -- "${(@)lines[2,-1]}"
  else
    _message -r "${lines[1]}"
  fi
}
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
      'save:save commands you already ran'
      'suggest:save a sequence you keep typing'
      'run:run a workflow'
      'resume:continue the last failed workflow'
      'list:list workflows'
      'show:print a workflow'
      'edit:change name, scope, parameters, steps'
      'rm:delete a workflow'
      'export:export as a bash script'
      'shim:run a workflow without typing fr'
      'unshim:undo shim'
    )
    (( ${#w} )) && _describe -V -t workflows 'workflow' w
    _describe -t commands 'command' c
  elif [[ $sub == (run|show|edit|rm|delete|remove|export|shim) && CURRENT == 3 ]]; then
    w=(${(f)"$(command fr _complete 2>/dev/null)"})
    (( ${#w} )) && _describe -V -t workflows 'workflow' w
  elif [[ $sub == unshim ]]; then
    compadd -- ${(f)"$(command fr shim 2>/dev/null)"}
  elif [[ $sub == run ]]; then
    __fr_complete_arg "$words[3]" $(( CURRENT - 4 ))
  elif [[ $sub != (list|ls|show|edit|rm|delete|remove|export|init|record|start|stop|status|save|suggest|resume|shim) ]]; then
    __fr_complete_arg "$sub" $(( CURRENT - 3 ))
  fi
}
if (( $+functions[compdef] )); then
  compdef _fr fr
  zstyle ':completion:*:*:fr:*' menu select
fi
