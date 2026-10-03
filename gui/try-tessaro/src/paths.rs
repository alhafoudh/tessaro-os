//! Where things are: what the app bundle carries, and the directory it keeps
//! the device in (docs/try-tessaro.md, "Where it keeps things").

use std::path::{Path, PathBuf};

/// The bundle's `Resources`, which `package-macos.sh` fills.
#[derive(Debug, Clone)]
pub struct Bundle {
    pub resources: PathBuf,
}

impl Bundle {
    /// `Contents/Resources` beside `Contents/MacOS/try-tessaro`, or
    /// `$TRY_TESSARO_RESOURCES`.
    pub fn locate() -> Result<Self, String> {
        if let Ok(dir) = std::env::var("TRY_TESSARO_RESOURCES") {
            return Ok(Self {
                resources: PathBuf::from(dir),
            });
        }
        let exe = std::env::current_exe().map_err(|err| format!("where am I: {err}"))?;
        let resources = exe
            .parent()
            .and_then(Path::parent)
            .map(|contents| contents.join("Resources"))
            .filter(|dir| dir.is_dir())
            .ok_or_else(|| {
                format!(
                    "{} is not inside Try Tessaro.app; run it with `mise run try:run`",
                    exe.display()
                )
            })?;
        Ok(Self { resources })
    }

    fn qemu(&self) -> PathBuf {
        self.resources.join("qemu")
    }

    pub fn qemu_system(&self) -> PathBuf {
        self.qemu().join("bin/qemu-system-aarch64")
    }

    pub fn qemu_img(&self) -> PathBuf {
        self.qemu().join("bin/qemu-img")
    }

    /// QEMU's data directory, for `-L`: the firmware and nothing it does
    /// not need.
    pub fn qemu_data(&self) -> PathBuf {
        self.qemu().join("share/qemu")
    }

    pub fn firmware(&self) -> PathBuf {
        self.qemu_data().join("edk2-aarch64-code.fd")
    }

    /// Where `tessaro-ctl` is, for the terminal's `PATH`.
    pub fn bin(&self) -> PathBuf {
        self.resources.join("bin")
    }

    /// `tessaro-gui`'s own app, nested in this one.
    pub fn gui_app(&self) -> PathBuf {
        self.resources.join("../Helpers/Tessaro.app")
    }

    /// The image the bundle ships: the one `.wic.zst` in `image/`.
    pub fn image(&self) -> Result<PathBuf, String> {
        let dir = self.resources.join("image");
        let found: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map_err(|err| format!("{}: {err}", dir.display()))?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.to_string_lossy().ends_with(".wic.zst"))
            .collect();
        match found.as_slice() {
            [one] => Ok(one.clone()),
            [] => Err(format!("{} holds no .wic.zst image", dir.display())),
            _ => Err(format!("{} holds more than one image", dir.display())),
        }
    }
}

/// Where the device lives between launches: `$TRY_TESSARO_DATA`, else the
/// platform's per-user application data.
pub fn data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("TRY_TESSARO_DATA") {
        return PathBuf::from(dir);
    }
    let home = || PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".to_string()));
    if cfg!(target_os = "macos") {
        home().join("Library/Application Support/Try Tessaro")
    } else if cfg!(windows) {
        std::env::var("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home())
            .join("Try Tessaro")
    } else {
        std::env::var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home().join(".local/share"))
            .join("try-tessaro")
    }
}
