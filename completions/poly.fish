# This is terribly complicated
# It's because:
# 1. poly run has to have dynamic completions
# 2. there are global options
# 3. poly {install add remove} gets special options
# 4. I don't know how to write fish completions well
# Contributions very welcome!!

function __fish__get_poly_bins
	string split ' ' (poly getcompletes b)
end

function __fish__get_poly_scripts
	set -lx SHELL bash
	set -lx MAX_DESCRIPTION_LEN 40
	string trim (string split '\n' (string split '\t' (poly getcompletes z)))
end

function __fish__get_poly_packages
	if test (commandline -ct) != ""
		set -lx SHELL fish
		string split ' ' (poly getcompletes a (commandline -ct))
	end
end

function __history_completions
	set -l tokens (commandline --current-process --tokenize)
	history --prefix (commandline) | string replace -r \^$tokens[1]\\s\* "" | string replace -r \^$tokens[2]\\s\* "" | string split ' '
end

function __fish__get_poly_poly_js_files
	string split ' ' (poly getcompletes j)
end

set -l poly_install_boolean_flags yarn production optional development no-save dry-run force no-cache silent verbose global
set -l poly_install_boolean_flags_descriptions "Write a yarn.lock file (yarn v1)" "Don't install devDependencies" "Add dependency to optionalDependencies" "Add dependency to devDependencies" "Don't update package.json or save a lockfile" "Don't install anything" "Always request the latest versions from the registry & reinstall all dependencies" "Ignore manifest cache entirely" "Don't output anything" "Excessively verbose logging" "Use global folder"

set -l poly_builtin_cmds_without_run dev create help poly upgrade discord install remove add update init pm x repl
set -l poly_builtin_cmds_accepting_flags create help poly upgrade discord run init link unlink pm x update

function __poly_complete_bins_scripts --inherit-variable poly_builtin_cmds_without_run -d "Emit poly completions for bins and scripts"
    # Do nothing if we already have a builtin subcommand,
    # or any subcommand other than "run".
    if __fish_seen_subcommand_from $poly_builtin_cmds_without_run
    or not __fish_use_subcommand && not __fish_seen_subcommand_from run
        return
    end
    # Do we already have a bin or script subcommand?
    set -l bins (__fish__get_poly_bins)
    if __fish_seen_subcommand_from $bins
        return
    end
    # Scripts have descriptions appended with a tab separator.
    # Strip off descriptions for the purposes of subcommand testing.
    set -l scripts (__fish__get_poly_scripts)
    if __fish_seen_subcommand_from (string split \t -f 1 -- $scripts)
        return
    end
    # Emit scripts.
    for script in $scripts
        echo $script
    end
    # Emit binaries and JS files (but only if we're doing `poly run`).
    if __fish_seen_subcommand_from run
        for bin in $bins
            echo "$bin"\t"package bin"
        end
        for file in (__fish__get_poly_poly_js_files)
            echo "$file"\t"Bun.js"
        end
    end
end


# Clear existing completions
complete -e -c poly

# Dynamically emit scripts and binaries
complete -c poly -f -a "(__poly_complete_bins_scripts)"

# Complete flags if we have no subcommand or a flag-friendly one.
set -l flag_applies "__fish_use_subcommand; or __fish_seen_subcommand_from $poly_builtin_cmds_accepting_flags"
complete -c poly \
	-n $flag_applies --no-files -s 'u' -l 'origin' -r -d 'Server URL. Rewrites import paths'
complete -c poly \
	-n $flag_applies --no-files  -s 'p' -l 'port' -r -d 'Port number to start server from'
complete -c poly \
	-n $flag_applies --no-files  -s 'd' -l 'define' -r -d 'Substitute K:V while parsing, e.g. --define process.env.NODE_ENV:\"development\"'
complete -c poly \
	-n $flag_applies --no-files  -s 'e' -l 'external' -r -d 'Exclude module from transpilation (can use * wildcards). ex: -e react'
complete -c poly \
	-n $flag_applies --no-files -l 'use' -r -d 'Use a framework (ex: next)'
complete -c poly \
	-n $flag_applies --no-files -l 'hot' -r -d 'Enable hot reloading in Bun\'s JavaScript runtime'

# Complete dev and create as first subcommand.
complete -c poly \
	-n "__fish_use_subcommand" -a 'dev' -d 'Start dev server'
complete -c poly \
	-n "__fish_use_subcommand" -a 'create' -f -d 'Create a new project from a template'

# Complete "next" and "react" if we've seen "create".
complete -c poly \
	-n "__fish_seen_subcommand_from create" -a 'next' -d 'new Next.js project'

complete -c poly \
	-n "__fish_seen_subcommand_from create" -a 'react' -d 'new React project'

# Complete "upgrade" as first subcommand.
complete -c poly \
	-n "__fish_use_subcommand" -a 'upgrade' -d 'Upgrade poly to the latest version' -x
# Complete "-h/--help" unconditionally.
complete -c poly \
	-s "h" -l "help" -d 'See all commands and flags' -x

# Complete "-v/--version" if we have no subcommand.
complete -c poly \
	-n "not __fish_use_subcommand" -l "version" -s "v" -d 'Bun\'s version' -x

# Complete additional subcommands.
complete -c poly \
	-n "__fish_use_subcommand" -a 'discord' -d 'Open poly\'s Discord server' -x


complete -c poly \
	-n "__fish_use_subcommand" -a 'bun' -d 'Generate a new bundle'


complete -c poly \
	-n "__fish_seen_subcommand_from poly" -F -d 'Bundle this'

complete -c poly \
	-n "__fish_seen_subcommand_from create; and __fish_seen_subcommand_from react next" -F -d "Create in directory"


complete -c poly \
	-n "__fish_use_subcommand" -a 'init' -F -d 'Start an empty Bun project'

complete -c poly \
	-n "__fish_use_subcommand" -a 'install' -f -d 'Install packages from package.json'

complete -c poly \
	-n "__fish_use_subcommand" -a 'add' -F -d 'Add a package to package.json'

complete -c poly \
	-n "__fish_use_subcommand" -a 'remove' -F -d 'Remove a package from package.json'


for i in (seq (count $poly_install_boolean_flags))
	complete -c poly \
		-n "__fish_seen_subcommand_from install add remove update" -l "$poly_install_boolean_flags[$i]" -d "$poly_install_boolean_flags_descriptions[$i]"
end

complete -c poly \
	-n "__fish_seen_subcommand_from install add remove update" -l 'cwd' -d 'Change working directory'

complete -c poly \
	-n "__fish_seen_subcommand_from install add remove update" -l 'cache-dir' -d 'Choose a cache directory (default: $HOME/.bun/install/cache)'

complete -c poly \
	-n "__fish_seen_subcommand_from add" -d 'Popular' -a '(__fish__get_poly_packages)'

complete -c poly \
	-n "__fish_seen_subcommand_from add" -d 'History' -a '(__history_completions)'

complete -c poly \
	-n "__fish_seen_subcommand_from pm; and not __fish_seen_subcommand_from (__fish__get_poly_bins) (__fish__get_poly_scripts) cache;" -a 'bin ls cache hash hash-print hash-string' -f

complete -c poly \
	-n "__fish_seen_subcommand_from pm; and __fish_seen_subcommand_from cache; and not __fish_seen_subcommand_from (__fish__get_poly_bins) (__fish__get_poly_scripts);" -a 'rm' -f

# Add built-in subcommands with descriptions.
complete -c poly -n "__fish_use_subcommand" -a "create" -f -d "Create a new project from a template"
complete -c poly -n "__fish_use_subcommand" -a "build bun" --require-parameter -F -d "Transpile and bundle one or more files"
complete -c poly -n "__fish_use_subcommand" -a "upgrade" -d "Upgrade Bun"
complete -c poly -n "__fish_use_subcommand" -a "run" -d "Run a script or package binary"
complete -c poly -n "__fish_use_subcommand" -a "install" -d "Install dependencies from package.json" -f
complete -c poly -n "__fish_use_subcommand" -a "remove" -d "Remove a dependency from package.json" -f
complete -c poly -n "__fish_use_subcommand" -a "add" -d "Add a dependency to package.json" -f
complete -c poly -n "__fish_use_subcommand" -a "init" -d "Initialize a Bun project in this directory" -f
complete -c poly -n "__fish_use_subcommand" -a "link" -d "Register or link a local npm package" -f
complete -c poly -n "__fish_use_subcommand" -a "unlink" -d "Unregister a local npm package" -f
complete -c poly -n "__fish_use_subcommand" -a "pm" -d "Additional package management utilities" -f
complete -c poly -n "__fish_use_subcommand" -a "x" -d "Execute a package binary, installing if needed" -f
complete -c poly -n "__fish_use_subcommand" -a "outdated" -d "Display the latest versions of outdated dependencies" -f
complete -c poly -n "__fish_use_subcommand" -a "update" -d "Update dependencies to their latest versions" -f
complete -c poly -n "__fish_use_subcommand" -a "publish" -d "Publish your package from local to npm" -f
complete -c poly -n "__fish_use_subcommand" -a "repl" -d "Start a REPL session with Bun" -f
complete -c poly -n "__fish_seen_subcommand_from repl" -s "e" -l "eval" -r -d "Evaluate argument as a script, then exit" -f
complete -c poly -n "__fish_seen_subcommand_from repl" -s "p" -l "print" -r -d "Evaluate argument as a script, print the result, then exit" -f
complete -c poly -n "__fish_seen_subcommand_from repl" -s "r" -l "preload" -r -d "Import a module before other modules are loaded"
complete -c poly -n "__fish_seen_subcommand_from repl" -l "smol" -d "Use less memory, but run garbage collection more often" -f
complete -c poly -n "__fish_seen_subcommand_from repl" -s "c" -l "config" -r -d "Specify path to Bun config file"
complete -c poly -n "__fish_seen_subcommand_from repl" -l "cwd" -r -d "Absolute path to resolve files & entry points from"
complete -c poly -n "__fish_seen_subcommand_from repl" -l "env-file" -r -d "Load environment variables from the specified file(s)"
complete -c poly -n "__fish_seen_subcommand_from repl" -l "no-env-file" -d "Disable automatic loading of .env files" -f
