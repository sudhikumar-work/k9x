# Print an optspec for argparse to handle cmd's options that are independent of any subcommand.
function __fish_k9x_global_optspecs
    string join \n x/context= n/namespace= A/all-namespaces r/readonly headless logoless crumbsless splashless write invert c/command= refresh= screen-dump-dir= h/help V/version
end

function __fish_k9x_needs_command
    # Figure out if the current invocation already has a command.
    set -l cmd (commandline -opc)
    set -e cmd[1]
    argparse -s (__fish_k9x_global_optspecs) -- $cmd 2>/dev/null
    or return
    if set -q argv[1]
        # Also print the command, so this can be used to figure out what it is.
        echo $argv[1]
        return 1
    end
    return 0
end

function __fish_k9x_using_subcommand
    set -l cmd (__fish_k9x_needs_command)
    test -z "$cmd"
    and return 1
    contains -- $cmd[1] $argv
end

complete -c k9x -n "__fish_k9x_needs_command" -s x -l context -d 'kube context to use (default: config or current-context)' -r
complete -c k9x -n "__fish_k9x_needs_command" -s n -l namespace -d 'namespace scope' -r
complete -c k9x -n "__fish_k9x_needs_command" -s c -l command -d 'initial resource view (same as positional VIEW)' -r
complete -c k9x -n "__fish_k9x_needs_command" -l refresh -d 'UI refresh tick in seconds (k9s --refresh)' -r
complete -c k9x -n "__fish_k9x_needs_command" -l screen-dump-dir -d 'directory for screendumps (k9s --screen-dump-dir)' -r
complete -c k9x -n "__fish_k9x_needs_command" -s A -l all-namespaces -d 'all namespaces'
complete -c k9x -n "__fish_k9x_needs_command" -s r -l readonly -d 'read-only mode: blocks every mutating action (TUI and CLI)'
complete -c k9x -n "__fish_k9x_needs_command" -l headless -d 'k9s parity: hide the header/info section'
complete -c k9x -n "__fish_k9x_needs_command" -l logoless -d 'k9s parity: hide the logo panel'
complete -c k9x -n "__fish_k9x_needs_command" -l crumbsless -d 'k9s parity: hide the shortcut hints'
complete -c k9x -n "__fish_k9x_needs_command" -l splashless -d 'k9s parity: accepted, k9x has no splash screen'
complete -c k9x -n "__fish_k9x_needs_command" -l write -d 'explicitly enable mutations (overrides readonly config)'
complete -c k9x -n "__fish_k9x_needs_command" -l invert -d 'swap dark/light theme presets'
complete -c k9x -n "__fish_k9x_needs_command" -s h -l help -d 'Print help'
complete -c k9x -n "__fish_k9x_needs_command" -s V -l version -d 'Print version'
complete -c k9x -n "__fish_k9x_needs_command" -a "completions" -d 'generate shell completions (bash|zsh|fish|elvish|powershell)'
complete -c k9x -n "__fish_k9x_needs_command" -a "ls" -d 'list resources (agent-friendly one-shot)'
complete -c k9x -n "__fish_k9x_needs_command" -a "get" -d 'fetch one object as yaml/json'
complete -c k9x -n "__fish_k9x_needs_command" -a "logs" -d 'stream pod logs'
complete -c k9x -n "__fish_k9x_needs_command" -a "describe" -d 'describe one object (+related events)'
complete -c k9x -n "__fish_k9x_needs_command" -a "decode" -d 'decode a secret to plaintext'
complete -c k9x -n "__fish_k9x_needs_command" -a "ctx" -d 'list contexts (or print current when none matches)'
complete -c k9x -n "__fish_k9x_needs_command" -a "ns" -d 'list namespaces'
complete -c k9x -n "__fish_k9x_needs_command" -a "del" -d 'delete an object (--yes required)'
complete -c k9x -n "__fish_k9x_needs_command" -a "scale" -d 'scale a workload (--yes required)'
complete -c k9x -n "__fish_k9x_needs_command" -a "restart" -d 'rollout restart deploy/sts/ds (--yes required)'
complete -c k9x -n "__fish_k9x_needs_command" -a "cordon" -d 'cordon a node (--yes required)'
complete -c k9x -n "__fish_k9x_needs_command" -a "uncordon" -d 'uncordon a node (--yes required)'
complete -c k9x -n "__fish_k9x_needs_command" -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c k9x -n "__fish_k9x_using_subcommand completions" -s h -l help -d 'Print help'
complete -c k9x -n "__fish_k9x_using_subcommand ls" -s n -r
complete -c k9x -n "__fish_k9x_using_subcommand ls" -s l -r
complete -c k9x -n "__fish_k9x_using_subcommand ls" -s o -r
complete -c k9x -n "__fish_k9x_using_subcommand ls" -s A
complete -c k9x -n "__fish_k9x_using_subcommand ls" -l watch
complete -c k9x -n "__fish_k9x_using_subcommand ls" -s h -l help -d 'Print help'
complete -c k9x -n "__fish_k9x_using_subcommand get" -s n -r
complete -c k9x -n "__fish_k9x_using_subcommand get" -s o -r
complete -c k9x -n "__fish_k9x_using_subcommand get" -s h -l help -d 'Print help'
complete -c k9x -n "__fish_k9x_using_subcommand logs" -s c -r
complete -c k9x -n "__fish_k9x_using_subcommand logs" -l tail -r
complete -c k9x -n "__fish_k9x_using_subcommand logs" -s f
complete -c k9x -n "__fish_k9x_using_subcommand logs" -s p
complete -c k9x -n "__fish_k9x_using_subcommand logs" -s t
complete -c k9x -n "__fish_k9x_using_subcommand logs" -s h -l help -d 'Print help'
complete -c k9x -n "__fish_k9x_using_subcommand describe" -s n -r
complete -c k9x -n "__fish_k9x_using_subcommand describe" -s h -l help -d 'Print help'
complete -c k9x -n "__fish_k9x_using_subcommand decode" -s n -r
complete -c k9x -n "__fish_k9x_using_subcommand decode" -s h -l help -d 'Print help'
complete -c k9x -n "__fish_k9x_using_subcommand ctx" -s h -l help -d 'Print help'
complete -c k9x -n "__fish_k9x_using_subcommand ns" -s h -l help -d 'Print help'
complete -c k9x -n "__fish_k9x_using_subcommand del" -l force
complete -c k9x -n "__fish_k9x_using_subcommand del" -l yes
complete -c k9x -n "__fish_k9x_using_subcommand del" -s h -l help -d 'Print help'
complete -c k9x -n "__fish_k9x_using_subcommand scale" -s n -r
complete -c k9x -n "__fish_k9x_using_subcommand scale" -l yes
complete -c k9x -n "__fish_k9x_using_subcommand scale" -s h -l help -d 'Print help'
complete -c k9x -n "__fish_k9x_using_subcommand restart" -s n -r
complete -c k9x -n "__fish_k9x_using_subcommand restart" -l yes
complete -c k9x -n "__fish_k9x_using_subcommand restart" -s h -l help -d 'Print help'
complete -c k9x -n "__fish_k9x_using_subcommand cordon" -l yes
complete -c k9x -n "__fish_k9x_using_subcommand cordon" -s h -l help -d 'Print help'
complete -c k9x -n "__fish_k9x_using_subcommand uncordon" -l yes
complete -c k9x -n "__fish_k9x_using_subcommand uncordon" -s h -l help -d 'Print help'
complete -c k9x -n "__fish_k9x_using_subcommand help; and not __fish_seen_subcommand_from completions ls get logs describe decode ctx ns del scale restart cordon uncordon help" -f -a "completions" -d 'generate shell completions (bash|zsh|fish|elvish|powershell)'
complete -c k9x -n "__fish_k9x_using_subcommand help; and not __fish_seen_subcommand_from completions ls get logs describe decode ctx ns del scale restart cordon uncordon help" -f -a "ls" -d 'list resources (agent-friendly one-shot)'
complete -c k9x -n "__fish_k9x_using_subcommand help; and not __fish_seen_subcommand_from completions ls get logs describe decode ctx ns del scale restart cordon uncordon help" -f -a "get" -d 'fetch one object as yaml/json'
complete -c k9x -n "__fish_k9x_using_subcommand help; and not __fish_seen_subcommand_from completions ls get logs describe decode ctx ns del scale restart cordon uncordon help" -f -a "logs" -d 'stream pod logs'
complete -c k9x -n "__fish_k9x_using_subcommand help; and not __fish_seen_subcommand_from completions ls get logs describe decode ctx ns del scale restart cordon uncordon help" -f -a "describe" -d 'describe one object (+related events)'
complete -c k9x -n "__fish_k9x_using_subcommand help; and not __fish_seen_subcommand_from completions ls get logs describe decode ctx ns del scale restart cordon uncordon help" -f -a "decode" -d 'decode a secret to plaintext'
complete -c k9x -n "__fish_k9x_using_subcommand help; and not __fish_seen_subcommand_from completions ls get logs describe decode ctx ns del scale restart cordon uncordon help" -f -a "ctx" -d 'list contexts (or print current when none matches)'
complete -c k9x -n "__fish_k9x_using_subcommand help; and not __fish_seen_subcommand_from completions ls get logs describe decode ctx ns del scale restart cordon uncordon help" -f -a "ns" -d 'list namespaces'
complete -c k9x -n "__fish_k9x_using_subcommand help; and not __fish_seen_subcommand_from completions ls get logs describe decode ctx ns del scale restart cordon uncordon help" -f -a "del" -d 'delete an object (--yes required)'
complete -c k9x -n "__fish_k9x_using_subcommand help; and not __fish_seen_subcommand_from completions ls get logs describe decode ctx ns del scale restart cordon uncordon help" -f -a "scale" -d 'scale a workload (--yes required)'
complete -c k9x -n "__fish_k9x_using_subcommand help; and not __fish_seen_subcommand_from completions ls get logs describe decode ctx ns del scale restart cordon uncordon help" -f -a "restart" -d 'rollout restart deploy/sts/ds (--yes required)'
complete -c k9x -n "__fish_k9x_using_subcommand help; and not __fish_seen_subcommand_from completions ls get logs describe decode ctx ns del scale restart cordon uncordon help" -f -a "cordon" -d 'cordon a node (--yes required)'
complete -c k9x -n "__fish_k9x_using_subcommand help; and not __fish_seen_subcommand_from completions ls get logs describe decode ctx ns del scale restart cordon uncordon help" -f -a "uncordon" -d 'uncordon a node (--yes required)'
complete -c k9x -n "__fish_k9x_using_subcommand help; and not __fish_seen_subcommand_from completions ls get logs describe decode ctx ns del scale restart cordon uncordon help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
