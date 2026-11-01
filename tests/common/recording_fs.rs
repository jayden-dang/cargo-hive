//! Records lock, remove and read calls against the real filesystem.
//! A configured path can refuse `flock`, report no file identity, or be replaced after the lock.
//! A configured directory is created after a lock is taken. Dropping a returned lock releases it.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use cargo_hive::platform::{self, Fs, HeldLock, TryLock};

#[derive(Default)]
struct Opts {
    unlockable: HashSet<PathBuf>,
    no_identity: HashSet<PathBuf>,
    replace_after_lock: HashSet<PathBuf>,
    create_dir_after_lock: Vec<PathBuf>,
}

struct State {
    fs: Box<dyn Fs>,
    log: Arc<Mutex<Vec<String>>>,
    opts: Mutex<Opts>,
}

#[derive(Clone)]
pub struct Recording {
    state: Rc<State>,
}

impl Recording {
    pub fn over_real() -> Recording {
        Recording {
            state: Rc::new(State {
                fs: platform::current().unwrap().fs,
                log: Arc::new(Mutex::new(Vec::new())),
                opts: Mutex::new(Opts::default()),
            }),
        }
    }

    pub fn unlockable(self, path: impl AsRef<Path>) -> Recording {
        self.state
            .opts
            .lock()
            .unwrap()
            .unlockable
            .insert(path.as_ref().to_path_buf());
        self
    }

    pub fn no_identity(self, path: impl AsRef<Path>) -> Recording {
        self.state
            .opts
            .lock()
            .unwrap()
            .no_identity
            .insert(path.as_ref().to_path_buf());
        self
    }

    pub fn replace_after_lock(self, path: impl AsRef<Path>) -> Recording {
        self.state
            .opts
            .lock()
            .unwrap()
            .replace_after_lock
            .insert(path.as_ref().to_path_buf());
        self
    }

    pub fn create_dir_after_lock(self, dir: impl AsRef<Path>) -> Recording {
        self.state
            .opts
            .lock()
            .unwrap()
            .create_dir_after_lock
            .push(dir.as_ref().to_path_buf());
        self
    }

    pub fn fs(&self) -> Box<dyn Fs> {
        Box::new(self.clone())
    }

    pub fn log(&self) -> Vec<String> {
        self.state.log.lock().unwrap().clone()
    }

    fn push(&self, line: String) {
        self.state.log.lock().unwrap().push(line);
    }

    fn flags(&self, path: &Path) -> (bool, bool, bool, Vec<PathBuf>) {
        let opts = self.state.opts.lock().unwrap();
        (
            opts.unlockable.contains(path),
            opts.no_identity.contains(path),
            opts.replace_after_lock.contains(path),
            opts.create_dir_after_lock.clone(),
        )
    }
}

impl Fs for Recording {
    fn temp_dir(&self, parent: &Path, prefix: &str) -> io::Result<platform::TempDir> {
        self.state.fs.temp_dir(parent, prefix)
    }

    fn write_file(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        self.state.fs.write_file(path, bytes)
    }

    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.push(format!("read {}", path.display()));
        self.state.fs.read(path)
    }

    fn hardlink(&self, src: &Path, dst: &Path) -> io::Result<()> {
        self.state.fs.hardlink(src, dst)
    }

    fn set_mtime(&self, path: &Path, t: SystemTime) -> io::Result<()> {
        self.state.fs.set_mtime(path, t)
    }

    fn mtime(&self, path: &Path) -> io::Result<SystemTime> {
        self.push(format!("read {}", path.display()));
        self.state.fs.mtime(path)
    }

    fn inode(&self, path: &Path) -> io::Result<u64> {
        self.state.fs.inode(path)
    }

    fn clone_file(&self, src: &Path, dst: &Path) -> io::Result<()> {
        self.state.fs.clone_file(src, dst)
    }

    fn sharing(&self, path: &Path) -> io::Result<platform::Sharing> {
        self.state.fs.sharing(path)
    }

    fn compress(&self, path: &Path) -> platform::CompressOutcome {
        self.state.fs.compress(path)
    }

    fn hold_lock(&self, kind: platform::LockKind, path: &Path) -> io::Result<platform::LockHolder> {
        self.state.fs.hold_lock(kind, path)
    }

    fn try_lock_exclusive(&self, path: &Path) -> TryLock {
        self.push(format!("lock {}", path.display()));
        let (unlockable, no_identity, replace, dirs) = self.flags(path);
        if unlockable {
            return TryLock::Unlockable(io::Error::other("flock unsupported"));
        }
        let locked = self.state.fs.try_lock_exclusive(path);
        let TryLock::Acquired(held) = locked else {
            return locked;
        };
        if replace {
            std::fs::remove_file(path).unwrap();
            std::fs::File::create(path).unwrap();
        }
        for dir in &dirs {
            std::fs::create_dir_all(dir).unwrap();
        }
        if no_identity {
            TryLock::Acquired(HeldLock::new(move || drop(held)))
        } else {
            TryLock::Acquired(held)
        }
    }

    fn try_lock_elsewhere(
        &self,
        kind: platform::LockKind,
        path: &Path,
    ) -> io::Result<platform::LockState> {
        self.state.fs.try_lock_elsewhere(kind, path)
    }

    fn entry_stat(&self, path: &Path) -> io::Result<platform::EntryStat> {
        self.push(format!("read {}", path.display()));
        self.state.fs.entry_stat(path)
    }

    fn read_dir_bounded(
        &self,
        dir: &Path,
        cap: usize,
    ) -> io::Result<Vec<(String, platform::EntryKind)>> {
        self.push(format!("read {}", dir.display()));
        self.state.fs.read_dir_bounded(dir, cap)
    }

    fn remove_tree(&self, path: &Path) -> io::Result<()> {
        self.push(format!("remove {}", path.display()));
        self.state.fs.remove_tree(path)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        self.push(format!("remove {}", path.display()));
        self.state.fs.remove_file(path)
    }

    fn volume(&self, path: &Path) -> io::Result<platform::VolumeInfo> {
        self.state.fs.volume(path)
    }
}
