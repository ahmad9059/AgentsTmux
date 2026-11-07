//! Sound effects for agent alerts: a chime when an agent finishes and a
//! double ping when it needs you (a question, a permission prompt, an error).
//!
//! The sounds are bundled in the binary and played with whichever command-line
//! player the system has. Everything here is best effort: a missing player or
//! an unwritable cache must never get in the way of a hook.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::tmux;

pub const SIDEBAR_SOUND: &str = "@sidebar_sound";
pub const SIDEBAR_SOUND_DONE: &str = "@sidebar_sound_done";
pub const SIDEBAR_SOUND_REQUEST: &str = "@sidebar_sound_request";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sound {
    /// An agent finished its work.
    Done,
    /// An agent needs attention: a question, a permission prompt, or an error.
    Request,
}

impl Sound {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "done" => Some(Self::Done),
            "request" => Some(Self::Request),
            _ => None,
        }
    }

    fn file_name(self) -> &'static str {
        match self {
            Self::Done => "done.wav",
            Self::Request => "request.wav",
        }
    }

    fn bundled(self) -> &'static [u8] {
        match self {
            Self::Done => include_bytes!("../assets/sounds/done.wav"),
            Self::Request => include_bytes!("../assets/sounds/request.wav"),
        }
    }

    fn option(self) -> &'static str {
        match self {
            Self::Done => SIDEBAR_SOUND_DONE,
            Self::Request => SIDEBAR_SOUND_REQUEST,
        }
    }
}

/// Whether sound effects are switched on (`@sidebar_sound`, default on).
pub fn enabled_in(options: &HashMap<String, String>) -> bool {
    !matches!(
        options
            .get(SIDEBAR_SOUND)
            .map(|v| v.trim().to_ascii_lowercase())
            .as_deref(),
        Some("off" | "0" | "false" | "no")
    )
}

/// Players tried in order, with the arguments that make each one quiet and
/// self-terminating. WAV is understood by all of them.
const PLAYERS: &[(&str, &[&str])] = &[
    ("pw-play", &[]),
    ("paplay", &[]),
    ("aplay", &["-q"]),
    ("afplay", &[]),
    ("mpv", &["--no-video", "--really-quiet"]),
    ("ffplay", &["-nodisp", "-autoexit", "-loglevel", "quiet"]),
    ("play", &["-q"]),
];

fn find_in_path(program: &str, path: &std::ffi::OsStr) -> Option<PathBuf> {
    std::env::split_paths(path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

/// The first available player and its arguments.
fn pick_player(path: &std::ffi::OsStr) -> Option<(PathBuf, &'static [&'static str])> {
    PLAYERS
        .iter()
        .find_map(|(name, args)| find_in_path(name, path).map(|found| (found, *args)))
}

fn cache_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))?;
    Some(
        base.join("agentstmux")
            .join("sounds")
            .join(env!("CARGO_PKG_VERSION")),
    )
}

/// Write the bundled sound to the cache once and return its path.
fn bundled_file(sound: Sound) -> Option<PathBuf> {
    let directory = cache_dir()?;
    let file = directory.join(sound.file_name());
    let bytes = sound.bundled();
    if std::fs::metadata(&file).map(|m| m.len()).ok() != Some(bytes.len() as u64) {
        std::fs::create_dir_all(&directory).ok()?;
        std::fs::write(&file, bytes).ok()?;
    }
    Some(file)
}

/// The file to play: a valid custom path from the tmux options, otherwise the
/// bundled sound.
fn resolve_file(sound: Sound, options: &HashMap<String, String>) -> Option<PathBuf> {
    options
        .get(sound.option())
        .map(|raw| expand_home(raw.trim()))
        .filter(|path| path.is_file())
        .or_else(|| bundled_file(sound))
}

fn expand_home(raw: &str) -> PathBuf {
    match (raw.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => Path::new(&home).join(rest),
        _ => PathBuf::from(raw),
    }
}

/// Play `sound` in the background and return immediately. Does nothing in
/// unit tests so they never make noise.
pub fn play(sound: Sound, options: &HashMap<String, String>) {
    if cfg!(test) {
        return;
    }
    let Some(path) = std::env::var_os("PATH") else {
        return;
    };
    let Some((player, args)) = pick_player(&path) else {
        return;
    };
    let Some(file) = resolve_file(sound, options) else {
        return;
    };
    spawn_detached(&player, args, &file);
}

fn spawn_detached(player: &Path, args: &[&str], file: &Path) {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let _ = Command::new(player)
        .args(args)
        .arg(file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // Own process group, so the sound finishes even if the hook is killed.
        .process_group(0)
        .spawn();
}

/// `agents-tmux play-sound <done|request>`: hear a sound with the current
/// settings, to check the player works.
pub fn cmd_play_sound(args: &[String]) -> i32 {
    let Some(sound) = args.first().and_then(|name| Sound::parse(name)) else {
        eprintln!("usage: agents-tmux play-sound <done|request>");
        return 2;
    };
    let options = tmux::get_all_global_options();
    let Some(path) = std::env::var_os("PATH") else {
        return 1;
    };
    let Some((player, player_args)) = pick_player(&path) else {
        eprintln!(
            "play-sound: no audio player found (tried pw-play, paplay, aplay, afplay, mpv, ffplay, play)"
        );
        return 1;
    };
    let Some(file) = resolve_file(sound, &options) else {
        eprintln!("play-sound: cannot prepare the sound file");
        return 1;
    };
    use std::process::{Command, Stdio};
    let status = Command::new(&player)
        .args(player_args)
        .arg(&file)
        .stdin(Stdio::null())
        .status();
    match status {
        Ok(status) if status.success() => 0,
        _ => {
            eprintln!("play-sound: {} failed", player.display());
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn sound_is_on_unless_switched_off() {
        assert!(enabled_in(&options(&[])));
        assert!(enabled_in(&options(&[(SIDEBAR_SOUND, "on")])));
        assert!(enabled_in(&options(&[(SIDEBAR_SOUND, "garbage")])));
        for off in ["off", "OFF", " 0 ", "false", "no"] {
            assert!(!enabled_in(&options(&[(SIDEBAR_SOUND, off)])), "{off}");
        }
    }

    #[test]
    fn bundled_sounds_are_valid_wav_files() {
        for sound in [Sound::Done, Sound::Request] {
            let bytes = sound.bundled();
            assert_eq!(&bytes[..4], b"RIFF");
            assert_eq!(&bytes[8..12], b"WAVE");
            assert!(
                bytes.len() > 10_000,
                "{sound:?} should not be silent filler"
            );
        }
    }

    #[test]
    fn parse_accepts_only_known_sounds() {
        assert_eq!(Sound::parse("done"), Some(Sound::Done));
        assert_eq!(Sound::parse("request"), Some(Sound::Request));
        assert_eq!(Sound::parse("loud"), None);
    }

    #[test]
    fn players_are_chosen_in_priority_order() {
        let directory = tempfile::tempdir().unwrap();
        for name in ["mpv", "paplay"] {
            std::fs::write(directory.path().join(name), "").unwrap();
        }
        let (player, args) = pick_player(directory.path().as_os_str()).unwrap();
        assert!(player.ends_with("paplay"), "{player:?}");
        assert!(args.is_empty());
        let empty = tempfile::tempdir().unwrap();
        assert!(pick_player(empty.path().as_os_str()).is_none());
    }

    #[test]
    fn a_custom_file_wins_and_a_missing_one_falls_back_to_the_bundled_sound() {
        let directory = tempfile::tempdir().unwrap();
        let custom = directory.path().join("mine.wav");
        std::fs::write(&custom, "x").unwrap();
        let chosen = resolve_file(
            Sound::Done,
            &options(&[(SIDEBAR_SOUND_DONE, custom.to_str().unwrap())]),
        );
        assert_eq!(chosen, Some(custom));

        // Point the cache at a temporary directory so the test stays hermetic.
        let cache = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CACHE_HOME", cache.path()) };
        let fallback = resolve_file(
            Sound::Request,
            &options(&[(SIDEBAR_SOUND_REQUEST, "/does/not/exist.wav")]),
        )
        .unwrap();
        assert!(fallback.starts_with(cache.path()));
        assert_eq!(
            std::fs::read(&fallback).unwrap(),
            Sound::Request.bundled(),
            "the bundled sound is written to the cache"
        );
    }
}
