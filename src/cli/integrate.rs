//! `integrate` subcommand — wires AgentsTmux into the coding agents installed
//! on this machine (Claude Code, Codex, OpenCode V2, Pi) without replacing
//! unrelated settings. Every step is idempotent and backs up a file before
//! changing it, so `agents-tmux.tmux` can run `integrate --auto` on every
//! tmux start and the plugin works right after installation.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use super::plugin_state::installed_plugin_status;
use super::setup::merge_agent_hooks;

type Step = fn(&Paths) -> Result<String, String>;

const CLAUDE_PLUGIN: &str = "agents-tmux@agentstmux";

#[derive(Debug, Clone)]
struct Paths {
    plugin_dir: PathBuf,
    state_dir: PathBuf,
    codex_home: PathBuf,
    opencode_dir: PathBuf,
    pi_dir: PathBuf,
    commandcode_dir: PathBuf,
}

impl Paths {
    fn from_env(plugin_dir: PathBuf) -> Option<Self> {
        let home = PathBuf::from(std::env::var_os("HOME")?);
        let dir = |var: &str, default: PathBuf| {
            std::env::var_os(var).map(PathBuf::from).unwrap_or(default)
        };
        Some(Self {
            plugin_dir,
            state_dir: home.join(".local/state/agentstmux"),
            codex_home: dir("CODEX_HOME", home.join(".codex")),
            opencode_dir: dir("XDG_CONFIG_HOME", home.join(".config")).join("opencode"),
            pi_dir: dir("PI_CODING_AGENT_DIR", home.join(".pi/agent")),
            commandcode_dir: home.join(".commandcode"),
        })
    }

    fn hook_script(&self) -> String {
        self.plugin_dir
            .join("hook.sh")
            .to_string_lossy()
            .into_owned()
    }
}

#[derive(Debug, Default)]
struct Options {
    all: bool,
    auto: bool,
    quiet: bool,
    claude: bool,
    codex: bool,
    opencode: bool,
    pi: bool,
    commandcode: bool,
    plugin_dir: Option<PathBuf>,
}

fn parse_args(args: &[String]) -> Result<Options, String> {
    let mut options = Options::default();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--all" => options.all = true,
            "--auto" => options.auto = true,
            "--quiet" => options.quiet = true,
            "--claude" => options.claude = true,
            "--codex" => options.codex = true,
            "--opencode" => options.opencode = true,
            "--pi" => options.pi = true,
            "--commandcode" => options.commandcode = true,
            "--plugin-dir" => {
                let value = iter.next().ok_or("--plugin-dir needs a path")?;
                options.plugin_dir = Some(PathBuf::from(value));
            }
            other => return Err(format!("unknown option: {other}")),
        }
    }
    if !(options.all
        || options.auto
        || options.claude
        || options.codex
        || options.opencode
        || options.commandcode
        || options.pi)
    {
        return Err(
            "select --auto, --all, or one or more of --claude --codex --opencode --pi --commandcode".into(),
        );
    }
    Ok(options)
}

/// The plugin root is the directory that holds `hook.sh`. Prefer the explicit
/// value from `agents-tmux.tmux` (which keeps the un-resolved TPM path), then
/// search upward from the running binary.
fn find_plugin_dir(explicit: Option<PathBuf>) -> Option<PathBuf> {
    let candidates = explicit
        .or_else(|| std::env::var_os("AGENTSTMUX_PLUGIN_DIR").map(PathBuf::from))
        .into_iter()
        .chain(std::env::current_exe().ok().into_iter().flat_map(|exe| {
            exe.ancestors()
                .skip(1)
                .take(4)
                .map(Path::to_path_buf)
                .collect::<Vec<_>>()
        }));
    candidates
        .into_iter()
        .find(|dir| dir.join("hook.sh").is_file())
}

pub(super) fn cmd_integrate(args: &[String]) -> i32 {
    let options = match parse_args(args) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("integrate: {message}");
            return 2;
        }
    };
    let Some(plugin_dir) = find_plugin_dir(options.plugin_dir.clone()) else {
        eprintln!("integrate: cannot locate the plugin directory (hook.sh); pass --plugin-dir");
        return 1;
    };
    let Some(paths) = Paths::from_env(plugin_dir) else {
        eprintln!("integrate: HOME is not set");
        return 1;
    };

    let wanted = |explicit: bool, detected: bool| {
        if options.all || explicit {
            true
        } else {
            options.auto && detected
        }
    };
    let steps: [(&str, bool, Step); 5] = [
        (
            "Command Code",
            wanted(options.commandcode, command_exists("commandcode")),
            commandcode,
        ),
        (
            "Codex",
            wanted(options.codex, paths.codex_home.is_dir()),
            codex,
        ),
        (
            "OpenCode V2",
            wanted(options.opencode, paths.opencode_dir.is_dir()),
            opencode,
        ),
        ("Pi", wanted(options.pi, paths.pi_dir.is_dir()), pi),
        (
            "Claude Code",
            wanted(options.claude, command_exists("claude")),
            claude,
        ),
    ];

    let mut failed = false;
    for (name, selected, run) in steps {
        if !selected {
            continue;
        }
        match run(&paths) {
            Ok(message) => {
                if !options.quiet && !message.is_empty() {
                    println!("{name}: {message}");
                }
            }
            Err(message) => {
                failed = true;
                eprintln!("{name}: {message}");
            }
        }
    }
    if failed && !options.auto { 1 } else { 0 }
}

fn command_exists(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).any(|dir| dir.join(name).is_file()))
        .unwrap_or(false)
}

fn backup(paths: &Paths, file: &Path) -> Result<(), String> {
    if !file.is_file() {
        return Ok(());
    }
    let directory = paths.state_dir.join("backups");
    fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    fs::copy(file, directory.join(format!("{stamp}-{name}"))).map_err(|e| e.to_string())?;
    Ok(())
}

fn load_json(path: &Path) -> Result<Value, String> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    match serde_json::from_str::<Value>(&text) {
        Ok(value) if value.is_object() => Ok(value),
        Ok(_) => Err(format!("{}: expected a JSON object", path.display())),
        Err(e) => Err(format!("{}: {e}; left untouched", path.display())),
    }
}

fn save_json(paths: &Paths, path: &Path, value: &Value) -> Result<(), String> {
    let parent = path.parent().ok_or("invalid path")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    backup(paths, path)?;
    let temporary = parent.join(format!(".agentstmux-{}.tmp", std::process::id()));
    let mut text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    text.push('\n');
    fs::write(&temporary, text).map_err(|e| e.to_string())?;
    if let Ok(meta) = fs::metadata(path) {
        let _ = fs::set_permissions(&temporary, meta.permissions());
    }
    fs::rename(&temporary, path).map_err(|e| e.to_string())
}

/// Point `link` at `target`. Returns whether anything changed. Refuses to
/// replace real files or directories.
fn link(target: &Path, link: &Path) -> Result<bool, String> {
    if let Ok(existing) = fs::read_link(link) {
        if existing == target {
            return Ok(false);
        }
        fs::remove_file(link).map_err(|e| e.to_string())?;
    } else if link.exists() {
        return Err(format!(
            "{} already exists and is not a symlink; remove it to continue",
            link.display()
        ));
    }
    if let Some(parent) = link.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    symlink(target, link).map_err(|e| e.to_string())?;
    Ok(true)
}

fn codex(paths: &Paths) -> Result<String, String> {
    let file = paths.codex_home.join("hooks.json");
    let mut config = load_json(&file)?;
    if !merge_agent_hooks("codex", &mut config, &paths.hook_script()) {
        return Ok(String::new());
    }
    save_json(paths, &file, &config)?;
    Ok("hooks added. Review and trust them with /hooks in Codex.".into())
}

fn opencode(paths: &Paths) -> Result<String, String> {
    let package = paths.opencode_dir.join("agents-tmux");
    let source = paths.plugin_dir.join("integrations/opencode-v2");
    let mut changed = link(&source, &package)?;

    let file = paths.opencode_dir.join("cli.json");
    let mut config = load_json(&file)?;
    let package_text = package.to_string_lossy().into_owned();
    let entry = json!({
        "package": package_text,
        "options": { "hookPath": paths.hook_script() },
    });
    let root = config.as_object_mut().ok_or("cli.json is not an object")?;
    root.entry("$schema")
        .or_insert_with(|| json!("https://opencode.ai/v2/cli.json"));
    let plugins = root.entry("plugins").or_insert_with(|| json!([]));
    let plugins = plugins
        .as_array_mut()
        .ok_or("cli.json plugins is not an array")?;
    let matches = |item: &Value| {
        item.as_str() == Some(package_text.as_str())
            || item.get("package").and_then(Value::as_str) == Some(package_text.as_str())
    };
    if plugins.iter().filter(|item| matches(item)).count() != 1 || !plugins.contains(&entry) {
        plugins.retain(|item| !matches(item));
        plugins.push(entry);
        save_json(paths, &file, &config)?;
        changed = true;
    }
    Ok(if changed {
        "configured. Use /reload if OpenCode is running.".into()
    } else {
        String::new()
    })
}

fn pi(paths: &Paths) -> Result<String, String> {
    let changed = link(
        &paths.plugin_dir.join("integrations/pi/agents-tmux.ts"),
        &paths.pi_dir.join("extensions/agents-tmux.ts"),
    )?;
    Ok(if changed {
        "extension installed. Reload extensions or start a new session.".into()
    } else {
        String::new()
    })
}

fn commandcode(paths: &Paths) -> Result<String, String> {
    let changed = link(
        &paths
            .plugin_dir
            .join("integrations/commandcode/agents-tmux.ts"),
        &paths.commandcode_dir.join("mods/agents-tmux.ts"),
    )?;
    Ok(if changed {
        "mod installed. Use /mods reload or start a new session.".into()
    } else {
        String::new()
    })
}

fn claude(paths: &Paths) -> Result<String, String> {
    if installed_plugin_status().installed {
        return Ok(String::new());
    }
    let run = |args: &[&str]| {
        Command::new("claude")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    };
    let directory = paths.plugin_dir.to_string_lossy().into_owned();
    if !run(&["plugin", "marketplace", "add", &directory])
        || !run(&["plugin", "install", CLAUDE_PLUGIN])
    {
        return Err(format!(
            "could not install {CLAUDE_PLUGIN}; run `claude plugin install {CLAUDE_PLUGIN}` manually"
        ));
    }
    Ok("plugin installed. Start a new Claude session to load it.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(root: &Path) -> Paths {
        let plugin_dir = root.join("plugin");
        fs::create_dir_all(plugin_dir.join("integrations/opencode-v2")).unwrap();
        fs::create_dir_all(plugin_dir.join("integrations/pi")).unwrap();
        fs::write(plugin_dir.join("hook.sh"), "").unwrap();
        Paths {
            plugin_dir,
            state_dir: root.join("state"),
            codex_home: root.join("codex"),
            opencode_dir: root.join("opencode"),
            pi_dir: root.join("pi"),
            commandcode_dir: root.join("commandcode"),
        }
    }

    #[test]
    fn codex_merge_keeps_existing_hooks_and_is_idempotent() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        fs::create_dir_all(&paths.codex_home).unwrap();
        let file = paths.codex_home.join("hooks.json");
        fs::write(
            &file,
            r#"{"hooks":{"Stop":[{"matcher":"","hooks":[{"type":"command","command":"echo keep"}]}]}}"#,
        )
        .unwrap();

        assert!(!codex(&paths).unwrap().is_empty());
        let first = fs::read_to_string(&file).unwrap();
        assert!(first.contains("echo keep"));
        assert!(first.contains("hook.sh codex stop"));
        assert!(codex(&paths).unwrap().is_empty());
        assert_eq!(fs::read_to_string(&file).unwrap(), first);
        assert_eq!(
            fs::read_dir(paths.state_dir.join("backups"))
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn opencode_preserves_settings_and_replaces_stale_entry_once() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        fs::create_dir_all(&paths.opencode_dir).unwrap();
        let file = paths.opencode_dir.join("cli.json");
        let package = paths.opencode_dir.join("agents-tmux");
        fs::write(
            &file,
            json!({
                "theme": {"name": "mine"},
                "plugins": ["other", {"package": package, "options": {"stale": true}}],
            })
            .to_string(),
        )
        .unwrap();

        assert!(!opencode(&paths).unwrap().is_empty());
        let config: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(config["theme"]["name"], "mine");
        assert_eq!(config["plugins"][0], "other");
        assert_eq!(config["plugins"].as_array().unwrap().len(), 2);
        assert_eq!(
            config["plugins"][1]["options"]["hookPath"],
            paths.hook_script()
        );
        assert_eq!(
            fs::read_link(&package).unwrap(),
            paths.plugin_dir.join("integrations/opencode-v2")
        );
        assert!(opencode(&paths).unwrap().is_empty());
    }

    #[test]
    fn opencode_creates_missing_config_and_pi_links_extension() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        assert!(!opencode(&paths).unwrap().is_empty());
        assert!(paths.opencode_dir.join("cli.json").is_file());
        assert!(!pi(&paths).unwrap().is_empty());
        assert!(pi(&paths).unwrap().is_empty());
        assert!(paths.pi_dir.join("extensions/agents-tmux.ts").is_symlink());
    }

    #[test]
    fn invalid_json_is_left_untouched() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        fs::create_dir_all(&paths.codex_home).unwrap();
        let file = paths.codex_home.join("hooks.json");
        fs::write(&file, "{ not json").unwrap();
        assert!(codex(&paths).is_err());
        assert_eq!(fs::read_to_string(&file).unwrap(), "{ not json");
    }

    #[test]
    fn real_file_is_never_replaced_by_a_link() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let extension = paths.pi_dir.join("extensions/agents-tmux.ts");
        fs::create_dir_all(extension.parent().unwrap()).unwrap();
        fs::write(&extension, "mine").unwrap();
        assert!(pi(&paths).is_err());
        assert_eq!(fs::read_to_string(&extension).unwrap(), "mine");
    }

    #[test]
    fn commandcode_mod_install_is_idempotent_and_keeps_settings() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        fs::create_dir_all(&paths.commandcode_dir).unwrap();
        let settings = paths.commandcode_dir.join("settings.json");
        fs::write(&settings, r#"{"model":"mine","hooks":{"Stop":[]}}"#).unwrap();
        let original = fs::read(&settings).unwrap();
        assert!(!commandcode(&paths).unwrap().is_empty());
        assert!(commandcode(&paths).unwrap().is_empty());
        assert_eq!(fs::read(&settings).unwrap(), original);
        let link = paths.commandcode_dir.join("mods/agents-tmux.ts");
        assert_eq!(
            fs::read_link(&link).unwrap(),
            paths
                .plugin_dir
                .join("integrations/commandcode/agents-tmux.ts")
        );
        fs::remove_file(&link).unwrap();
        fs::write(&link, "user's mod").unwrap();
        assert!(commandcode(&paths).is_err());
        assert_eq!(fs::read_to_string(&link).unwrap(), "user's mod");
    }

    #[test]
    fn argument_parsing_requires_a_selection() {
        let args = |items: &[&str]| items.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(parse_args(&args(&[])).is_err());
        assert!(parse_args(&args(&["--bogus"])).is_err());
        assert!(parse_args(&args(&["--auto", "--quiet"])).unwrap().auto);
    }
}
