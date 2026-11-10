# fr shell integration (fish 3.1+). Add to ~/.config/fish/config.fish:
#   fr init fish | source

set -g FR_SESSION ""
set -q FR_RESUME; or set -g FR_RESUME
set -q FR_RESUME_STEP; or set -g FR_RESUME_STEP
set -g __fr_depth 0
set -g __fr_dir $PWD

function __fr_start
    if test -n "$FR_SESSION"
        echo "error: already recording — run `fr` or `fr stop` to finish" >&2
        return 1
    end
    set -g FR_SESSION (command fr record fish)
    or begin
        set -g FR_SESSION ""
        return 1
    end
    if functions -q fish_right_prompt
        functions -c fish_right_prompt __fr_right_prompt
    end
    function fish_right_prompt
        set_color red
        echo -n '● rec'
        set_color normal
        if functions -q __fr_right_prompt
            echo -n ' '
            __fr_right_prompt
        end
    end
    echo "  ● Recording. Run your commands, then `fr` again to save." >&2
end

function __fr_stop
    if test -z "$FR_SESSION"
        echo "error: not recording — run `fr` to start" >&2
        return 1
    end
    set -l f $FR_SESSION
    set -g FR_SESSION ""
    functions -e fish_right_prompt
    if functions -q __fr_right_prompt
        functions -c __fr_right_prompt fish_right_prompt
        functions -e __fr_right_prompt
    end
    command fr stop $f
end

# Replay scripts end by calling this to hand their status back.
function __fr_status
    return $argv[1]
end

# Replayed steps run in *this* shell so cd and set -gx persist.
function __fr_run
    set -g __fr_depth (math $__fr_depth + 1)
    set -l __fr_s (command fr _exec fish $__fr_depth $argv | string collect -N)
    set -l rc $pipestatus[1]
    if test $rc -ne 0 -o -z "$__fr_s"
        set -g __fr_depth (math $__fr_depth - 1)
        return $rc
    end
    set -l __fr_i
    set -l __fr_rc 0
    eval $__fr_s
    set rc $status
    set -g __fr_depth (math $__fr_depth - 1)
    return $rc
end

function __fr_resume
    if test -z "$FR_RESUME"
        echo "error: nothing to resume — the last workflow did not fail" >&2
        return 1
    end
    __fr_run --from $FR_RESUME_STEP $FR_RESUME
end

# Replayed steps go into history, so ↑ and Ctrl-R find them (fish 4+).
function __fr_remember
    builtin history append -- $argv[1] 2>/dev/null
    return 0
end

# `fr shim next` makes `next` mean `fr next`.
function __fr_shim
    eval "function $argv[1]; fr $argv[1] \$argv; end"
end

function fr
    set -l sub $argv[1]
    switch "$sub"
        case ''
            if test -n "$FR_SESSION"
                __fr_stop
            else
                __fr_start
            end
        case record start
            __fr_start
        case stop
            __fr_stop
        case status
            if test -n "$FR_SESSION"
                echo recording
            else
                echo "not recording"
            end
        case run
            set -e argv[1]
            __fr_run $argv
        case resume
            __fr_resume
        case save
            set -e argv[1]
            command fr _save fish $fish_pid $argv
        case suggest
            command fr _suggest fish
        case shim
            command fr $argv
            or return
            set -e argv[1]
            for n in $argv
                __fr_shim $n
            end
        case unshim
            command fr $argv
            or return
            set -e argv[1]
            for n in $argv
                functions -e $n
            end
        case list ls show edit rm delete remove export init version help -h --help -V --version
            command fr $argv
        case '*'
            __fr_run $argv
    end
end

# Hooks: log every finished command with its status, using only builtins.
# Set FR_NO_HISTORY to stop logging.
function __fr_preexec --on-event fish_preexec
    set -g __fr_dir $PWD
end

function __fr_postexec --on-event fish_postexec
    set -l s $status
    set -l cmd $argv[1]
    test -n "$cmd"; or return $s
    if test -n "$FR_SESSION"
        command fr _ev $FR_SESSION $s $cmd
    end
    if not set -q FR_NO_HISTORY
        set -l c (string replace -a '\\' '\\\\' -- $cmd | string replace -a \t '\t' | string join '\n')
        set -l d (string replace -a '\\' '\\\\' -- $__fr_dir | string replace -a \t '\t' | string join '\n')
        printf '\t%s\t%s\t%s\t%s\n' $fish_pid $s "$d" "$c" >>$__fr_log 2>/dev/null
    end
    return $s
end

set -l __fr_log_dir (string replace -r '/[^/]*$' '' -- $__fr_log)
test -d $__fr_log_dir; or mkdir -p $__fr_log_dir 2>/dev/null

# Completion: workflows ranked for this folder, then commands; choices for arguments.
function __fr_complete_args
    set -l words (commandline -opc)
    set -l name $words[2]
    set -l index (math (count $words) - 2)
    if test "$name" = run
        set name $words[3]
        set index (math (count $words) - 3)
    end
    command fr _complete $name $index 2>/dev/null | tail -n +2
end
complete -c fr -f
complete -c fr -n __fish_use_subcommand -a '(command fr _complete 2>/dev/null | string replace -r ":" \t)'
complete -c fr -n __fish_use_subcommand -a 'record stop status save suggest run resume list show edit rm export shim unshim'
complete -c fr -n 'not __fish_use_subcommand; and __fish_seen_subcommand_from run show edit rm export shim' -a '(command fr _complete 2>/dev/null | string replace -r ":" \t)'
complete -c fr -n 'not __fish_use_subcommand; and not __fish_seen_subcommand_from list ls show edit rm export shim unshim init record stop status save suggest resume' -a '(__fr_complete_args)'
