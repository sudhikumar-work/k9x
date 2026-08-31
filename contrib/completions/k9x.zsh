#compdef k9x

autoload -U is-at-least

_k9x() {
    typeset -A opt_args
    typeset -a _arguments_options
    local ret=1

    if is-at-least 5.2; then
        _arguments_options=(-s -S -C)
    else
        _arguments_options=(-s -C)
    fi

    local context curcontext="$curcontext" state line
    _arguments "${_arguments_options[@]}" : \
'-x+[kube context to use (default\: config or current-context)]:CONTEXT:_default' \
'--context=[kube context to use (default\: config or current-context)]:CONTEXT:_default' \
'-n+[namespace scope]:NAMESPACE:_default' \
'--namespace=[namespace scope]:NAMESPACE:_default' \
'-c+[initial resource view (same as positional VIEW)]:COMMAND:_default' \
'--command=[initial resource view (same as positional VIEW)]:COMMAND:_default' \
'--refresh=[UI refresh tick in seconds (k9s --refresh)]:REFRESH:_default' \
'--screen-dump-dir=[directory for screendumps (k9s --screen-dump-dir)]:SCREEN_DUMP_DIR:_default' \
'-A[all namespaces]' \
'--all-namespaces[all namespaces]' \
'-r[read-only mode\: blocks every mutating action (TUI and CLI)]' \
'--readonly[read-only mode\: blocks every mutating action (TUI and CLI)]' \
'--headless[k9s parity\: hide the header/info section]' \
'--logoless[k9s parity\: hide the logo panel]' \
'--crumbsless[k9s parity\: hide the shortcut hints]' \
'--splashless[k9s parity\: accepted, k9x has no splash screen]' \
'--write[explicitly enable mutations (overrides readonly config)]' \
'--invert[swap dark/light theme presets]' \
'-h[Print help]' \
'--help[Print help]' \
'-V[Print version]' \
'--version[Print version]' \
'::view -- initial resource view (e.g. po, deploy, svc):_default' \
":: :_k9x_commands" \
"*::: :->k9x" \
&& ret=0
    case $state in
    (k9x)
        words=($line[2] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:k9x-command-$line[2]:"
        case $line[2] in
            (completions)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
':shell:(bash elvish fish powershell zsh)' \
&& ret=0
;;
(ls)
_arguments "${_arguments_options[@]}" : \
'-n+[]:NAMESPACE:_default' \
'-l+[]:SELECTOR:_default' \
'-o+[]:OUTPUT:_default' \
'-A[]' \
'--watch[]' \
'-h[Print help]' \
'--help[Print help]' \
':resource:_default' \
&& ret=0
;;
(get)
_arguments "${_arguments_options[@]}" : \
'-n+[]:NAMESPACE:_default' \
'-o+[]:OUTPUT:_default' \
'-h[Print help]' \
'--help[Print help]' \
':resource:_default' \
':name:_default' \
&& ret=0
;;
(logs)
_arguments "${_arguments_options[@]}" : \
'-c+[]:CONTAINER:_default' \
'--tail=[]:TAIL:_default' \
'-f[]' \
'-p[]' \
'-t[]' \
'-h[Print help]' \
'--help[Print help]' \
':pod:_default' \
&& ret=0
;;
(describe)
_arguments "${_arguments_options[@]}" : \
'-n+[]:NAMESPACE:_default' \
'-h[Print help]' \
'--help[Print help]' \
':resource:_default' \
':name:_default' \
&& ret=0
;;
(decode)
_arguments "${_arguments_options[@]}" : \
'-n+[]:NAMESPACE:_default' \
'-h[Print help]' \
'--help[Print help]' \
':name:_default' \
&& ret=0
;;
(ctx)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
'::context:_default' \
&& ret=0
;;
(ns)
_arguments "${_arguments_options[@]}" : \
'-h[Print help]' \
'--help[Print help]' \
&& ret=0
;;
(del)
_arguments "${_arguments_options[@]}" : \
'--force[]' \
'--yes[]' \
'-h[Print help]' \
'--help[Print help]' \
':resource:_default' \
':name:_default' \
&& ret=0
;;
(scale)
_arguments "${_arguments_options[@]}" : \
'-n+[]:NAMESPACE:_default' \
'--yes[]' \
'-h[Print help]' \
'--help[Print help]' \
':resource:_default' \
':name:_default' \
':replicas:_default' \
&& ret=0
;;
(restart)
_arguments "${_arguments_options[@]}" : \
'-n+[]:NAMESPACE:_default' \
'--yes[]' \
'-h[Print help]' \
'--help[Print help]' \
':resource:_default' \
':name:_default' \
&& ret=0
;;
(cordon)
_arguments "${_arguments_options[@]}" : \
'--yes[]' \
'-h[Print help]' \
'--help[Print help]' \
':node:_default' \
&& ret=0
;;
(uncordon)
_arguments "${_arguments_options[@]}" : \
'--yes[]' \
'-h[Print help]' \
'--help[Print help]' \
':node:_default' \
&& ret=0
;;
(help)
_arguments "${_arguments_options[@]}" : \
":: :_k9x__subcmd__help_commands" \
"*::: :->help" \
&& ret=0

    case $state in
    (help)
        words=($line[1] "${words[@]}")
        (( CURRENT += 1 ))
        curcontext="${curcontext%:*:*}:k9x-help-command-$line[1]:"
        case $line[1] in
            (completions)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(ls)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(get)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(logs)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(describe)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(decode)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(ctx)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(ns)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(del)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(scale)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(restart)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(cordon)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(uncordon)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
(help)
_arguments "${_arguments_options[@]}" : \
&& ret=0
;;
        esac
    ;;
esac
;;
        esac
    ;;
esac
}

(( $+functions[_k9x_commands] )) ||
_k9x_commands() {
    local commands; commands=(
'completions:generate shell completions (bash|zsh|fish|elvish|powershell)' \
'ls:list resources (agent-friendly one-shot)' \
'get:fetch one object as yaml/json' \
'logs:stream pod logs' \
'describe:describe one object (+related events)' \
'decode:decode a secret to plaintext' \
'ctx:list contexts (or print current when none matches)' \
'ns:list namespaces' \
'del:delete an object (--yes required)' \
'scale:scale a workload (--yes required)' \
'restart:rollout restart deploy/sts/ds (--yes required)' \
'cordon:cordon a node (--yes required)' \
'uncordon:uncordon a node (--yes required)' \
'help:Print this message or the help of the given subcommand(s)' \
    )
    _describe -t commands 'k9x commands' commands "$@"
}
(( $+functions[_k9x__subcmd__completions_commands] )) ||
_k9x__subcmd__completions_commands() {
    local commands; commands=()
    _describe -t commands 'k9x completions commands' commands "$@"
}
(( $+functions[_k9x__subcmd__cordon_commands] )) ||
_k9x__subcmd__cordon_commands() {
    local commands; commands=()
    _describe -t commands 'k9x cordon commands' commands "$@"
}
(( $+functions[_k9x__subcmd__ctx_commands] )) ||
_k9x__subcmd__ctx_commands() {
    local commands; commands=()
    _describe -t commands 'k9x ctx commands' commands "$@"
}
(( $+functions[_k9x__subcmd__decode_commands] )) ||
_k9x__subcmd__decode_commands() {
    local commands; commands=()
    _describe -t commands 'k9x decode commands' commands "$@"
}
(( $+functions[_k9x__subcmd__del_commands] )) ||
_k9x__subcmd__del_commands() {
    local commands; commands=()
    _describe -t commands 'k9x del commands' commands "$@"
}
(( $+functions[_k9x__subcmd__describe_commands] )) ||
_k9x__subcmd__describe_commands() {
    local commands; commands=()
    _describe -t commands 'k9x describe commands' commands "$@"
}
(( $+functions[_k9x__subcmd__get_commands] )) ||
_k9x__subcmd__get_commands() {
    local commands; commands=()
    _describe -t commands 'k9x get commands' commands "$@"
}
(( $+functions[_k9x__subcmd__help_commands] )) ||
_k9x__subcmd__help_commands() {
    local commands; commands=(
'completions:generate shell completions (bash|zsh|fish|elvish|powershell)' \
'ls:list resources (agent-friendly one-shot)' \
'get:fetch one object as yaml/json' \
'logs:stream pod logs' \
'describe:describe one object (+related events)' \
'decode:decode a secret to plaintext' \
'ctx:list contexts (or print current when none matches)' \
'ns:list namespaces' \
'del:delete an object (--yes required)' \
'scale:scale a workload (--yes required)' \
'restart:rollout restart deploy/sts/ds (--yes required)' \
'cordon:cordon a node (--yes required)' \
'uncordon:uncordon a node (--yes required)' \
'help:Print this message or the help of the given subcommand(s)' \
    )
    _describe -t commands 'k9x help commands' commands "$@"
}
(( $+functions[_k9x__subcmd__help__subcmd__completions_commands] )) ||
_k9x__subcmd__help__subcmd__completions_commands() {
    local commands; commands=()
    _describe -t commands 'k9x help completions commands' commands "$@"
}
(( $+functions[_k9x__subcmd__help__subcmd__cordon_commands] )) ||
_k9x__subcmd__help__subcmd__cordon_commands() {
    local commands; commands=()
    _describe -t commands 'k9x help cordon commands' commands "$@"
}
(( $+functions[_k9x__subcmd__help__subcmd__ctx_commands] )) ||
_k9x__subcmd__help__subcmd__ctx_commands() {
    local commands; commands=()
    _describe -t commands 'k9x help ctx commands' commands "$@"
}
(( $+functions[_k9x__subcmd__help__subcmd__decode_commands] )) ||
_k9x__subcmd__help__subcmd__decode_commands() {
    local commands; commands=()
    _describe -t commands 'k9x help decode commands' commands "$@"
}
(( $+functions[_k9x__subcmd__help__subcmd__del_commands] )) ||
_k9x__subcmd__help__subcmd__del_commands() {
    local commands; commands=()
    _describe -t commands 'k9x help del commands' commands "$@"
}
(( $+functions[_k9x__subcmd__help__subcmd__describe_commands] )) ||
_k9x__subcmd__help__subcmd__describe_commands() {
    local commands; commands=()
    _describe -t commands 'k9x help describe commands' commands "$@"
}
(( $+functions[_k9x__subcmd__help__subcmd__get_commands] )) ||
_k9x__subcmd__help__subcmd__get_commands() {
    local commands; commands=()
    _describe -t commands 'k9x help get commands' commands "$@"
}
(( $+functions[_k9x__subcmd__help__subcmd__help_commands] )) ||
_k9x__subcmd__help__subcmd__help_commands() {
    local commands; commands=()
    _describe -t commands 'k9x help help commands' commands "$@"
}
(( $+functions[_k9x__subcmd__help__subcmd__logs_commands] )) ||
_k9x__subcmd__help__subcmd__logs_commands() {
    local commands; commands=()
    _describe -t commands 'k9x help logs commands' commands "$@"
}
(( $+functions[_k9x__subcmd__help__subcmd__ls_commands] )) ||
_k9x__subcmd__help__subcmd__ls_commands() {
    local commands; commands=()
    _describe -t commands 'k9x help ls commands' commands "$@"
}
(( $+functions[_k9x__subcmd__help__subcmd__ns_commands] )) ||
_k9x__subcmd__help__subcmd__ns_commands() {
    local commands; commands=()
    _describe -t commands 'k9x help ns commands' commands "$@"
}
(( $+functions[_k9x__subcmd__help__subcmd__restart_commands] )) ||
_k9x__subcmd__help__subcmd__restart_commands() {
    local commands; commands=()
    _describe -t commands 'k9x help restart commands' commands "$@"
}
(( $+functions[_k9x__subcmd__help__subcmd__scale_commands] )) ||
_k9x__subcmd__help__subcmd__scale_commands() {
    local commands; commands=()
    _describe -t commands 'k9x help scale commands' commands "$@"
}
(( $+functions[_k9x__subcmd__help__subcmd__uncordon_commands] )) ||
_k9x__subcmd__help__subcmd__uncordon_commands() {
    local commands; commands=()
    _describe -t commands 'k9x help uncordon commands' commands "$@"
}
(( $+functions[_k9x__subcmd__logs_commands] )) ||
_k9x__subcmd__logs_commands() {
    local commands; commands=()
    _describe -t commands 'k9x logs commands' commands "$@"
}
(( $+functions[_k9x__subcmd__ls_commands] )) ||
_k9x__subcmd__ls_commands() {
    local commands; commands=()
    _describe -t commands 'k9x ls commands' commands "$@"
}
(( $+functions[_k9x__subcmd__ns_commands] )) ||
_k9x__subcmd__ns_commands() {
    local commands; commands=()
    _describe -t commands 'k9x ns commands' commands "$@"
}
(( $+functions[_k9x__subcmd__restart_commands] )) ||
_k9x__subcmd__restart_commands() {
    local commands; commands=()
    _describe -t commands 'k9x restart commands' commands "$@"
}
(( $+functions[_k9x__subcmd__scale_commands] )) ||
_k9x__subcmd__scale_commands() {
    local commands; commands=()
    _describe -t commands 'k9x scale commands' commands "$@"
}
(( $+functions[_k9x__subcmd__uncordon_commands] )) ||
_k9x__subcmd__uncordon_commands() {
    local commands; commands=()
    _describe -t commands 'k9x uncordon commands' commands "$@"
}

if [ "$funcstack[1]" = "_k9x" ]; then
    _k9x "$@"
else
    compdef _k9x k9x
fi
