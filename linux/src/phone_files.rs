use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::os::unix::fs::FileExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fuser::{
    BackgroundSession, Config, Errno, FileAttr, FileHandle, FileType, Filesystem, FopenFlags, Generation, INodeNo,
    LockOwner, MountOption, OpenFlags, RenameFlags, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty,
    ReplyEntry, ReplyOpen, ReplyStatfs, ReplyWrite, Request, TimeOrNow, WriteFlags,
};
use phonelink_engine::api::engine::Engine;
use phonelink_engine::api::phone_files::{PhoneFileError, PhoneFileFailure, PhoneFileItem, PhoneFileProgress};
use tokio::runtime::Handle;

const ROOT: u64 = 1;
const TTL: Duration = Duration::from_secs(1);

pub fn mount_point() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join("PhoneLink")
}

fn errno(error: &PhoneFileError) -> Errno {
    let PhoneFileError::Failure { failure, .. } = error;
    match failure {
        PhoneFileFailure::UnknownItem | PhoneFileFailure::FileNotFound => Errno::ENOENT,
        PhoneFileFailure::Exists => Errno::EEXIST,
        PhoneFileFailure::NotSupported | PhoneFileFailure::Ignored => Errno::ENOTSUP,
        PhoneFileFailure::PermissionRequired | PhoneFileFailure::LtwOff | PhoneFileFailure::PcLocked => Errno::EACCES,
        PhoneFileFailure::InProgress => Errno::EBUSY,
        PhoneFileFailure::Cancelled => Errno::EINTR,
        _ => Errno::EIO,
    }
}

struct Silent;

impl PhoneFileProgress for Silent {
    fn progress(&self, _done: u64, _total: u64) {}
}

#[derive(Default)]
struct Inodes {
    by_ino: HashMap<u64, String>,
    by_id: HashMap<String, u64>,
    next: u64,
}

impl Inodes {
    fn of(&mut self, identifier: &str) -> u64 {
        if identifier.is_empty() {
            return ROOT;
        }
        if let Some(ino) = self.by_id.get(identifier) {
            return *ino;
        }
        self.next = self.next.max(ROOT) + 1;
        self.by_ino.insert(self.next, identifier.to_owned());
        self.by_id.insert(identifier.to_owned(), self.next);
        self.next
    }

    fn identifier(&self, ino: u64) -> Option<String> {
        if ino == ROOT { Some(String::new()) } else { self.by_ino.get(&ino).cloned() }
    }

    fn rebind(&mut self, ino: u64, identifier: &str) {
        if let Some(old) = self.by_ino.insert(ino, identifier.to_owned()) {
            self.by_id.remove(&old);
        }
        self.by_id.insert(identifier.to_owned(), ino);
    }
}

enum Open {
    Read(File),
    Write { file: File, path: PathBuf, parent: String, name: String, existing: Option<String>, ino: u64 },
}

pub struct PhoneFs {
    engine: Arc<Engine>,
    runtime: Handle,
    domain: String,
    inodes: Mutex<Inodes>,
    opened: Mutex<HashMap<u64, Open>>,
    next_handle: AtomicU64,
    uid: u32,
    gid: u32,
}

fn time(seconds: Option<i64>) -> SystemTime {
    seconds.and_then(|s| u64::try_from(s).ok()).map_or(UNIX_EPOCH, |s| UNIX_EPOCH + Duration::from_secs(s))
}

fn temporary(name: &str) -> PathBuf {
    let unique = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    std::env::temp_dir().join(format!("phonelink-{unique}-{}", name.replace('/', "_")))
}

impl PhoneFs {
    fn files(&self) -> Arc<phonelink_engine::api::phone_files::PhoneFilesApi> {
        self.engine.clone().phone_files()
    }

    fn ino(&self, identifier: &str) -> u64 {
        self.inodes.lock().unwrap_or_else(PoisonError::into_inner).of(identifier)
    }

    fn identifier(&self, ino: INodeNo) -> Result<String, Errno> {
        self.inodes.lock().unwrap_or_else(PoisonError::into_inner).identifier(ino.0).ok_or(Errno::ENOENT)
    }

    fn attr(&self, ino: u64, directory: bool, size: u64, created: SystemTime, modified: SystemTime) -> FileAttr {
        FileAttr {
            ino: INodeNo(ino),
            size,
            blocks: size.div_ceil(512),
            atime: modified,
            mtime: modified,
            ctime: modified,
            crtime: created,
            kind: if directory { FileType::Directory } else { FileType::RegularFile },
            perm: if directory { 0o755 } else { 0o644 },
            nlink: if directory { 2 } else { 1 },
            uid: self.uid,
            gid: self.gid,
            rdev: 0,
            blksize: 4096,
            flags: 0,
        }
    }

    fn item_attr(&self, item: &PhoneFileItem) -> FileAttr {
        let ino = self.ino(&item.identifier);
        let size = u64::try_from(item.size).unwrap_or_default();
        self.attr(ino, item.is_directory, size, time(item.created), time(item.modified))
    }

    fn root_attr(&self) -> FileAttr {
        self.attr(ROOT, true, 0, UNIX_EPOCH, SystemTime::now())
    }

    fn child(&self, parent: INodeNo, name: &OsStr) -> Result<PhoneFileItem, Errno> {
        let parent = self.identifier(parent)?;
        let name = name.to_string_lossy();
        self.files()
            .children(self.domain.clone(), parent)
            .map_err(|e| errno(&e))?
            .into_iter()
            .find(|item| item.name == name)
            .ok_or(Errno::ENOENT)
    }

    fn handle(&self, open: Open) -> FileHandle {
        let handle = self.next_handle.fetch_add(1, Ordering::Relaxed);
        self.opened.lock().unwrap_or_else(PoisonError::into_inner).insert(handle, open);
        FileHandle(handle)
    }

    fn download(&self, identifier: &str) -> Result<PathBuf, Errno> {
        let files = self.files();
        let request = format!("fuse-{}", self.next_handle.fetch_add(1, Ordering::Relaxed));
        let content = self
            .runtime
            .block_on(files.download(self.domain.clone(), identifier.to_owned(), request, Arc::new(Silent)))
            .map_err(|e| errno(&e))?;
        Ok(PathBuf::from(content.path))
    }

    fn finish_write(&self, open: Open) -> Result<(), Errno> {
        let Open::Write { file, path, parent, name, existing, ino } = open else { return Ok(()) };
        drop(file);
        let files = self.files();
        if let Some(existing) = existing {
            self.runtime.block_on(files.delete(self.domain.clone(), existing)).map_err(|e| errno(&e))?;
        }
        let request = format!("fuse-{}", self.next_handle.fetch_add(1, Ordering::Relaxed));
        let source = path.to_string_lossy().into_owned();
        let uploaded = self
            .runtime
            .block_on(files.upload(self.domain.clone(), parent, name, source, None, None, request))
            .map_err(|e| errno(&e));
        let _ = std::fs::remove_file(&path);
        let item = uploaded?;
        self.inodes.lock().unwrap_or_else(PoisonError::into_inner).rebind(ino, &item.identifier);
        Ok(())
    }
}

impl Filesystem for PhoneFs {
    fn lookup(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        match self.child(parent, name) {
            Ok(item) => reply.entry(&TTL, &self.item_attr(&item), Generation(0)),
            Err(e) => reply.error(e),
        }
    }

    fn getattr(&self, _req: &Request, ino: INodeNo, _fh: Option<FileHandle>, reply: ReplyAttr) {
        if ino.0 == ROOT {
            return reply.attr(&TTL, &self.root_attr());
        }
        let identifier = match self.identifier(ino) {
            Ok(identifier) => identifier,
            Err(e) => return reply.error(e),
        };
        match self.files().item(self.domain.clone(), identifier) {
            Ok(item) => reply.attr(&TTL, &self.item_attr(&item)),
            Err(_) => reply.attr(&TTL, &self.attr(ino.0, false, 0, UNIX_EPOCH, SystemTime::now())),
        }
    }

    fn setattr(
        &self,
        _req: &Request,
        ino: INodeNo,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<TimeOrNow>,
        _mtime: Option<TimeOrNow>,
        _ctime: Option<SystemTime>,
        fh: Option<FileHandle>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<fuser::BsdFileFlags>,
        reply: ReplyAttr,
    ) {
        if let (Some(size), Some(fh)) = (size, fh)
            && let Some(Open::Write { file, .. }) =
                self.opened.lock().unwrap_or_else(PoisonError::into_inner).get(&fh.0)
        {
            let _ = file.set_len(size);
            return reply.attr(&TTL, &self.attr(ino.0, false, size, SystemTime::now(), SystemTime::now()));
        }
        self.getattr(_req, ino, fh, reply);
    }

    fn readdir(&self, _req: &Request, ino: INodeNo, _fh: FileHandle, offset: u64, mut reply: ReplyDirectory) {
        let identifier = match self.identifier(ino) {
            Ok(identifier) => identifier,
            Err(e) => return reply.error(e),
        };
        let children = match self.files().children(self.domain.clone(), identifier) {
            Ok(children) => children,
            Err(e) => return reply.error(errno(&e)),
        };
        let mut entries =
            vec![(ino.0, FileType::Directory, ".".to_owned()), (ROOT, FileType::Directory, "..".to_owned())];
        entries.extend(children.into_iter().map(|item| {
            let kind = if item.is_directory { FileType::Directory } else { FileType::RegularFile };
            (self.ino(&item.identifier), kind, item.name)
        }));
        for (index, (entry, kind, name)) in entries.into_iter().enumerate().skip(offset as usize) {
            if reply.add(INodeNo(entry), index as u64 + 1, kind, name) {
                break;
            }
        }
        reply.ok();
    }

    fn open(&self, _req: &Request, ino: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
        let identifier = match self.identifier(ino) {
            Ok(identifier) => identifier,
            Err(e) => return reply.error(e),
        };
        let writing = flags.0 & libc::O_ACCMODE != libc::O_RDONLY;
        let truncate = flags.0 & libc::O_TRUNC != 0;
        let path = if writing && truncate {
            let path = temporary("upload");
            if let Err(e) = File::create(&path) {
                eprintln!("a phone file was not opened: {e}");
                return reply.error(Errno::EIO);
            }
            path
        } else {
            match self.download(&identifier) {
                Ok(path) => path,
                Err(e) => return reply.error(e),
            }
        };
        let file = match OpenOptions::new().read(true).write(writing).open(&path) {
            Ok(file) => file,
            Err(_) => return reply.error(Errno::EIO),
        };
        let open = if writing {
            let item = match self.files().item(self.domain.clone(), identifier.clone()) {
                Ok(item) => item,
                Err(e) => return reply.error(errno(&e)),
            };
            Open::Write { file, path, parent: item.parent, name: item.name, existing: Some(identifier), ino: ino.0 }
        } else {
            let _ = std::fs::remove_file(&path);
            Open::Read(file)
        };
        reply.opened(self.handle(open), FopenFlags::empty());
    }

    fn read(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyData,
    ) {
        let opened = self.opened.lock().unwrap_or_else(PoisonError::into_inner);
        let file = match opened.get(&fh.0) {
            Some(Open::Read(file)) | Some(Open::Write { file, .. }) => file,
            None => return reply.error(Errno::EBADF),
        };
        let mut buffer = vec![0; size as usize];
        match file.read_at(&mut buffer, offset) {
            Ok(read) => reply.data(&buffer[..read]),
            Err(_) => reply.error(Errno::EIO),
        }
    }

    fn write(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        data: &[u8],
        _write_flags: WriteFlags,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyWrite,
    ) {
        let opened = self.opened.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(Open::Write { file, .. }) = opened.get(&fh.0) else { return reply.error(Errno::EBADF) };
        match file.write_all_at(data, offset) {
            Ok(()) => reply.written(data.len() as u32),
            Err(_) => reply.error(Errno::EIO),
        }
    }

    fn release(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        let open = self.opened.lock().unwrap_or_else(PoisonError::into_inner).remove(&fh.0);
        match open.map(|open| self.finish_write(open)) {
            Some(Err(e)) => reply.error(e),
            _ => reply.ok(),
        }
    }

    fn create(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        _flags: i32,
        reply: ReplyCreate,
    ) {
        let parent_id = match self.identifier(parent) {
            Ok(identifier) => identifier,
            Err(e) => return reply.error(e),
        };
        let name = name.to_string_lossy().into_owned();
        let path = temporary(&name);
        let file = match OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&path) {
            Ok(file) => file,
            Err(_) => return reply.error(Errno::EIO),
        };
        let ino = self.ino(&format!("pending:{parent_id}/{name}"));
        let attr = self.attr(ino, false, 0, SystemTime::now(), SystemTime::now());
        let handle = self.handle(Open::Write { file, path, parent: parent_id, name, existing: None, ino });
        reply.created(&TTL, &attr, Generation(0), handle, FopenFlags::empty());
    }

    fn mkdir(&self, _req: &Request, parent: INodeNo, name: &OsStr, _mode: u32, _umask: u32, reply: ReplyEntry) {
        let parent = match self.identifier(parent) {
            Ok(identifier) => identifier,
            Err(e) => return reply.error(e),
        };
        let files = self.files();
        let name = name.to_string_lossy().into_owned();
        match self.runtime.block_on(files.create_folder(self.domain.clone(), parent, name)) {
            Ok(item) => reply.entry(&TTL, &self.item_attr(&item), Generation(0)),
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn unlink(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        let item = match self.child(parent, name) {
            Ok(item) => item,
            Err(e) => return reply.error(e),
        };
        let files = self.files();
        match self.runtime.block_on(files.delete(self.domain.clone(), item.identifier)) {
            Ok(()) => reply.ok(),
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn rmdir(&self, req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        self.unlink(req, parent, name, reply);
    }

    fn rename(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        newparent: INodeNo,
        newname: &OsStr,
        _flags: RenameFlags,
        reply: ReplyEmpty,
    ) {
        let item = match self.child(parent, name) {
            Ok(item) => item,
            Err(e) => return reply.error(e),
        };
        let target = match self.identifier(newparent) {
            Ok(identifier) => identifier,
            Err(e) => return reply.error(e),
        };
        let files = self.files();
        let newname = newname.to_string_lossy().into_owned();
        match self.runtime.block_on(files.rename(self.domain.clone(), item.identifier, target, newname)) {
            Ok(_) => reply.ok(),
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn statfs(&self, _req: &Request, _ino: INodeNo, reply: ReplyStatfs) {
        let state = self.files().state().ok();
        let block = 4096u64;
        let total = state.as_ref().map_or(0, |s| u64::try_from(s.storage_total).unwrap_or_default()) / block;
        let free = state.as_ref().map_or(0, |s| u64::try_from(s.storage_free).unwrap_or_default()) / block;
        reply.statfs(total, free, free, 0, 0, block as u32, 255, block as u32);
    }
}

pub fn mount(engine: Arc<Engine>, runtime: Handle, domain: String) -> Option<BackgroundSession> {
    let point = mount_point();
    let _ = std::process::Command::new("fusermount3").arg("-u").arg("-q").arg(&point).status();
    if let Err(e) = std::fs::create_dir_all(&point) {
        eprintln!("the phone files folder {} was not made: {e}", point.display());
        return None;
    }
    let filesystem = PhoneFs {
        engine,
        runtime,
        domain,
        inodes: Mutex::default(),
        opened: Mutex::default(),
        next_handle: AtomicU64::new(1),
        uid: unsafe { libc::getuid() },
        gid: unsafe { libc::getgid() },
    };
    let mut config = Config::default();
    config.mount_options = vec![
        MountOption::FSName("phonelink".to_owned()),
        MountOption::Subtype("phonelink".to_owned()),
        MountOption::NoExec,
        MountOption::NoSuid,
        MountOption::NoDev,
    ];
    fuser::spawn_mount(filesystem, &point, &config)
        .inspect_err(|e| eprintln!("the phone files were not mounted at {}: {e}", point.display()))
        .ok()
}
