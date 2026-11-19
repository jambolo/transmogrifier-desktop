//! Desktop shortcuts to one app. A shortcut starts Transmogrifier with `--open <project id>`. When Transmogrifier
//! is already running, the single-instance plugin hands that launch's arguments to the running copy instead.
//!
//! Linux gets a `.desktop` entry and Windows a `.lnk` shell link; both point at the running executable.

use std::path::{Path, PathBuf};

/// The argument that names the app to open.
pub const OPEN_ARG: &str = "--open";

/// The project a launch asks for, from its command line (program name first).
pub fn requested_project(args: &[String]) -> Option<String> {
    let mut args = args.iter().skip(1);
    while let Some(arg) = args.next() {
        if arg == OPEN_ARG {
            return args.next().filter(|id| is_project_id(id)).cloned();
        }
    }
    None
}

/// Project IDs are UUIDs; anything else is refused so it can't smuggle extra arguments into a shortcut.
fn is_project_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// What the shortcut runs: the AppImage when running from one (its own executable lives in a temporary mount),
/// otherwise this executable.
fn program() -> Result<PathBuf, String> {
    #[cfg(target_os = "linux")]
    if let Some(image) = std::env::var_os("APPIMAGE") {
        return Ok(image.into());
    }
    std::env::current_exe().map_err(|e| e.to_string())
}

/// Puts a shortcut to the project on the desktop and returns its path. `data_dir` holds the icon a Linux
/// shortcut refers to.
pub fn create(desktop: &Path, data_dir: &Path, project_id: &str, name: &str) -> Result<PathBuf, String> {
    if !is_project_id(project_id) {
        return Err("This app's ID can't be used in a shortcut".to_string());
    }
    std::fs::create_dir_all(desktop).map_err(|e| e.to_string())?;
    platform::create(desktop, data_dir, &program()?, project_id, name)
}

#[cfg(target_os = "linux")]
mod platform {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};

    use super::OPEN_ARG;

    const ICON: &[u8] = include_bytes!("../icons/128x128@2x.png");

    pub fn create(desktop: &Path, data_dir: &Path, program: &Path, project_id: &str, name: &str) -> Result<PathBuf, String> {
        let icon = data_dir.join("shortcut-icon.png");
        fs::write(&icon, ICON).map_err(|e| e.to_string())?;
        // The file name is the project's, so making a shortcut again replaces the old one.
        let path = desktop.join(format!("transmogrifier-{project_id}.desktop"));
        fs::write(&path, desktop_entry(program, &icon, project_id, name)).map_err(|e| e.to_string())?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
        // GNOME launches desktop files only once they are marked trusted; elsewhere `gio` may be missing.
        let _ = Command::new("gio")
            .args(["set", "-t", "string"])
            .arg(&path)
            .args(["metadata::trusted", "true"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        Ok(path)
    }

    /// A desktop entry that opens the project. See the freedesktop.org Desktop Entry Specification.
    pub(super) fn desktop_entry(program: &Path, icon: &Path, project_id: &str, name: &str) -> String {
        let name: String = name.chars().filter(|c| !c.is_control()).collect();
        let name = if name.trim().is_empty() {
            "Transmogrifier app"
        } else {
            name.trim()
        };
        let exec = format!("{} {OPEN_ARG} {project_id}", exec_arg(&program.to_string_lossy()));
        format!(
            "[Desktop Entry]\nType=Application\nVersion=1.0\nName={}\nExec={}\nIcon={}\nTerminal=false\nStartupNotify=true\n",
            string_value(name),
            string_value(&exec),
            string_value(&icon.to_string_lossy()),
        )
    }

    /// Escapes a value of type string.
    fn string_value(value: &str) -> String {
        value.replace('\\', "\\\\")
    }

    /// Quotes one argument of `Exec`, before the string escaping that applies to the whole value.
    fn exec_arg(arg: &str) -> String {
        let mut quoted = String::from("\"");
        for c in arg.chars() {
            if matches!(c, '"' | '`' | '$' | '\\') {
                quoted.push('\\');
            }
            quoted.push(c);
        }
        quoted.push('"');
        quoted.replace('%', "%%")
    }
}

#[cfg(windows)]
mod platform {
    use std::path::{Path, PathBuf};

    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows::core::{HSTRING, Interface};

    use super::OPEN_ARG;

    pub fn create(desktop: &Path, _data_dir: &Path, program: &Path, project_id: &str, name: &str) -> Result<PathBuf, String> {
        let path = free_path(desktop, &crate::safe_file_name(name, "app"));
        save_link(&path, program, &format!("{OPEN_ARG} {project_id}"), name).map_err(|e| e.message())?;
        Ok(path)
    }

    /// `<name>.lnk`, or `<name> (2).lnk` and so on, so an existing file is never replaced.
    fn free_path(desktop: &Path, name: &str) -> PathBuf {
        let mut path = desktop.join(format!("{name}.lnk"));
        let mut n = 2;
        while path.exists() {
            path = desktop.join(format!("{name} ({n}).lnk"));
            n += 1;
        }
        path
    }

    fn save_link(path: &Path, program: &Path, arguments: &str, description: &str) -> windows::core::Result<()> {
        // SAFETY: COM is initialized on this thread for the duration of the calls and released afterwards.
        unsafe {
            let initialized = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
            let result = (|| {
                let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
                link.SetPath(&HSTRING::from(program))?;
                link.SetArguments(&HSTRING::from(arguments))?;
                link.SetIconLocation(&HSTRING::from(program), 0)?;
                link.SetDescription(&HSTRING::from(description))?;
                if let Some(dir) = program.parent() {
                    link.SetWorkingDirectory(&HSTRING::from(dir))?;
                }
                link.cast::<IPersistFile>()?.Save(&HSTRING::from(path), true)
            })();
            if initialized {
                CoUninitialize();
            }
            result
        }
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
mod platform {
    use std::path::{Path, PathBuf};

    pub fn create(_desktop: &Path, _data_dir: &Path, _program: &Path, _project_id: &str, _name: &str) -> Result<PathBuf, String> {
        Err("Desktop shortcuts aren't available on this system".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| arg.to_string()).collect()
    }

    #[test]
    fn a_launch_names_the_project_after_the_open_argument() {
        let id = "0f8fad5b-d9cb-469f-a165-70867728950e";
        assert_eq!(
            requested_project(&args(&["transmogrifier", "--open", id])),
            Some(id.to_string())
        );
        assert_eq!(requested_project(&args(&["transmogrifier"])), None);
        assert_eq!(requested_project(&args(&["transmogrifier", "--open"])), None);
        assert_eq!(requested_project(&args(&["--open", id])), None);
        assert_eq!(requested_project(&args(&["transmogrifier", "--open", "a b"])), None);
    }

    #[test]
    fn shortcuts_refuse_ids_that_are_not_project_ids() {
        let dir = std::env::temp_dir();
        assert!(create(&dir, &dir, "a\nb", "x").is_err());
        assert!(create(&dir, &dir, "", "x").is_err());
    }

    #[cfg(windows)]
    #[test]
    fn windows_shortcuts_never_replace_an_existing_file() {
        let desktop = std::env::temp_dir().join(format!("transmogrifier-shortcut-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&desktop);
        let first = create(&desktop, &desktop, "p1", "Tip: jar").unwrap();
        let second = create(&desktop, &desktop, "p1", "Tip: jar").unwrap();
        assert_eq!(first, desktop.join("Tip_ jar.lnk"));
        assert_eq!(second, desktop.join("Tip_ jar (2).lnk"));
        assert!(std::fs::metadata(&first).unwrap().len() > 0);
        std::fs::remove_dir_all(&desktop).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn desktop_entries_quote_and_escape_their_values() {
        let entry = platform::desktop_entry(
            Path::new("/opt/My \"Apps\"/$x\\100%"),
            Path::new("/data/icon.png"),
            "p1",
            "Tip\njar \\ 1",
        );
        assert_eq!(
            entry,
            "[Desktop Entry]\nType=Application\nVersion=1.0\nName=Tipjar \\\\ 1\n\
             Exec=\"/opt/My \\\\\"Apps\\\\\"/\\\\$x\\\\\\\\100%%\" --open p1\n\
             Icon=/data/icon.png\nTerminal=false\nStartupNotify=true\n"
        );
    }
}
