//! Records lock, remove and read calls against the real filesystem.
//! A configured path can refuse `flock`, report no file identity, or be replaced after the lock.
//! A configured directory is created after a lock is taken. Dropping a returned lock releases it.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use cargo_hive::platform::{self, Fs, HeldLock, TryLock};

type OnCompress = Arc<dyn Fn(&Path) + Send + Sync>;

#[derive(Default)]
struct Opts {
    unlockable: HashSet<PathBuf>,
    no_identity: HashSet<PathBuf>,
    replace_after_lock: HashSet<PathBuf>,
    create_dir_after_lock: Vec<PathBuf>,
    /// After this many successful removes, later removes fail. `Some(0)` fails the first.
    fail_removes_after: Option<u64>,
    fail_remove: HashSet<PathBuf>,
    fail_sharing: HashSet<PathBuf>,
    fail_stat: HashSet<PathBuf>,
    fail_clone: HashMap<PathBuf, i32>,
    plant_before_clone: HashMap<PathBuf, Vec<u8>>,
    /// After the first `entry_stat` of this path, replace the file with a symlink to the value.
    symlink_after_stat: HashMap<PathBuf, PathBuf>,
    on_compress: Option<OnCompress>,
    /// Create `child` just before the 1-based `nth` bounded listing of `parent`.
    create_before_listing: Vec<(PathBuf, PathBuf, u64)>,
}

struct State {
    fs: Box<dyn Fs>,
    log: Arc<Mutex<Vec<String>>>,
    opts: Mutex<Opts>,
    removes: Mutex<u64>,
    listings: Mutex<HashMap<PathBuf, u64>>,
    stated: Mutex<HashSet<PathBuf>>,
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
                removes: Mutex::new(0),
                listings: Mutex::new(HashMap::new()),
                stated: Mutex::new(HashSet::new()),
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

    /// Creates `child` immediately before the `nth` bounded listing of `parent`.
    pub fn create_before_listing(
        self,
        parent: impl AsRef<Path>,
        child: impl AsRef<Path>,
        nth: u64,
    ) -> Recording {
        self.state.opts.lock().unwrap().create_before_listing.push((
            parent.as_ref().to_path_buf(),
            child.as_ref().to_path_buf(),
            nth,
        ));
        self
    }

    /// `entry_stat` of `path` fails. Listing the parent still succeeds.
    pub fn fail_stat(self, path: impl AsRef<Path>) -> Recording {
        self.state
            .opts
            .lock()
            .unwrap()
            .fail_stat
            .insert(path.as_ref().to_path_buf());
        self
    }

    /// The first `entry_stat` still sees the real file. It is then replaced with a symlink.
    pub fn symlink_after_stat(self, path: impl AsRef<Path>, dest: impl AsRef<Path>) -> Recording {
        self.state
            .opts
            .lock()
            .unwrap()
            .symlink_after_stat
            .insert(path.as_ref().to_path_buf(), dest.as_ref().to_path_buf());
        self
    }

    pub fn fail_clone(self, dst: impl AsRef<Path>, errno: i32) -> Recording {
        self.state
            .opts
            .lock()
            .unwrap()
            .fail_clone
            .insert(dst.as_ref().to_path_buf(), errno);
        self
    }

    pub fn plant_before_clone(self, dst: impl AsRef<Path>, bytes: &[u8]) -> Recording {
        self.state
            .opts
            .lock()
            .unwrap()
            .plant_before_clone
            .insert(dst.as_ref().to_path_buf(), bytes.to_vec());
        self
    }

    pub fn on_compress(self, f: impl Fn(&Path) + Send + Sync + 'static) -> Recording {
        self.state.opts.lock().unwrap().on_compress = Some(Arc::new(f));
        self
    }

    /// `sharing` of `path` fails. A stat of that path still succeeds.
    pub fn fail_sharing(self, path: impl AsRef<Path>) -> Recording {
        self.state
            .opts
            .lock()
            .unwrap()
            .fail_sharing
            .insert(path.as_ref().to_path_buf());
        self
    }

    /// The first `n` removes succeed. Every later `remove_file` or `remove_tree` returns an error.
    pub fn fail_removes_after(self, n: usize) -> Recording {
        self.state.opts.lock().unwrap().fail_removes_after = Some(n as u64);
        self
    }

    /// `remove_file` and `remove_tree` of this path fail. Other paths are removed.
    pub fn fail_remove(self, path: impl AsRef<Path>) -> Recording {
        self.state
            .opts
            .lock()
            .unwrap()
            .fail_remove
            .insert(path.as_ref().to_path_buf());
        self
    }

    fn refuse_remove(&self, path: &Path) -> bool {
        if self.state.opts.lock().unwrap().fail_remove.contains(path) {
            return true;
        }
        let after = self.state.opts.lock().unwrap().fail_removes_after;
        let Some(after) = after else {
            return false;
        };
        let mut seen = self.state.removes.lock().unwrap();
        if *seen >= after {
            true
        } else {
            *seen += 1;
            false
        }
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
        let dir = self.state.fs.temp_dir(parent, prefix)?;
        self.push(format!("temp_dir {}", dir.path().display()));
        Ok(dir)
    }

    fn write_file(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        self.push(format!("write_file {}", path.display()));
        self.state.fs.write_file(path, bytes)
    }

    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.push(format!("read {}", path.display()));
        self.state.fs.read(path)
    }

    fn hardlink(&self, src: &Path, dst: &Path) -> io::Result<()> {
        self.push(format!("hardlink {}", dst.display()));
        self.state.fs.hardlink(src, dst)
    }

    fn set_mtime(&self, path: &Path, t: SystemTime) -> io::Result<()> {
        self.push(format!("set_mtime {}", path.display()));
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
        self.push(format!("clone {} -> {}", src.display(), dst.display()));
        let (errno, planted) = {
            let opts = self.state.opts.lock().unwrap();
            (
                opts.fail_clone.get(dst).copied(),
                opts.plant_before_clone.get(dst).cloned(),
            )
        };
        if let Some(errno) = errno {
            return Err(io::Error::from_raw_os_error(errno));
        }
        if let Some(bytes) = planted {
            std::fs::write(dst, bytes)?;
        }
        self.state.fs.clone_file(src, dst)
    }

    fn create_dir(&self, path: &Path) -> io::Result<()> {
        self.push(format!("create_dir {}", path.display()));
        self.state.fs.create_dir(path)
    }

    fn sharing(&self, path: &Path) -> io::Result<platform::Sharing> {
        if self.state.opts.lock().unwrap().fail_sharing.contains(path) {
            return Err(io::Error::other("sharing attributes could not be read"));
        }
        self.state.fs.sharing(path)
    }

    fn compress(&self, path: &Path) -> platform::CompressOutcome {
        self.push(format!("compress {}", path.display()));
        let hook = self.state.opts.lock().unwrap().on_compress.clone();
        if let Some(hook) = hook {
            hook(path);
        }
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
        let shown = path.display().to_string();
        let log = Arc::clone(&self.state.log);
        let identity = if no_identity { None } else { held.identity() };
        let release = move || {
            log.lock().unwrap().push(format!("unlock {shown}"));
            drop(held);
        };
        match identity {
            Some((dev, ino)) => TryLock::Acquired(HeldLock::with_identity(dev, ino, release)),
            None => TryLock::Acquired(HeldLock::new(release)),
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
        if self.state.opts.lock().unwrap().fail_stat.contains(path) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("injected stat failure for {}", path.display()),
            ));
        }
        let stat = self.state.fs.entry_stat(path)?;
        let dest = self
            .state
            .opts
            .lock()
            .unwrap()
            .symlink_after_stat
            .get(path)
            .cloned();
        if let Some(dest) = dest {
            let mut stated = self.state.stated.lock().unwrap();
            if stated.insert(path.to_path_buf()) {
                std::fs::remove_file(path)
                    .unwrap_or_else(|err| panic!("replace {} after stat: {err}", path.display()));
                std::os::unix::fs::symlink(&dest, path)
                    .unwrap_or_else(|err| panic!("symlink {} after stat: {err}", path.display()));
            }
        }
        Ok(stat)
    }

    fn read_dir_bounded(
        &self,
        dir: &Path,
        cap: usize,
    ) -> io::Result<Vec<(String, platform::EntryKind)>> {
        self.push(format!("read {}", dir.display()));
        let nth = {
            let mut listings = self.state.listings.lock().unwrap();
            let seen = listings.entry(dir.to_path_buf()).or_insert(0);
            *seen += 1;
            *seen
        };
        let pending = self
            .state
            .opts
            .lock()
            .unwrap()
            .create_before_listing
            .clone();
        for (parent, child, at) in pending {
            if parent == dir && at == nth {
                std::fs::create_dir_all(child).unwrap();
            }
        }
        self.state.fs.read_dir_bounded(dir, cap)
    }

    fn remove_tree(&self, path: &Path) -> io::Result<()> {
        self.push(format!("remove {}", path.display()));
        if self.refuse_remove(path) {
            return Err(io::Error::other("simulated stop"));
        }
        self.state.fs.remove_tree(path)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        self.push(format!("remove {}", path.display()));
        if self.refuse_remove(path) {
            return Err(io::Error::other("simulated stop"));
        }
        self.state.fs.remove_file(path)
    }

    fn volume(&self, path: &Path) -> io::Result<platform::VolumeInfo> {
        self.state.fs.volume(path)
    }
}
