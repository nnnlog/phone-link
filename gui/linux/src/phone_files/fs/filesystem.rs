//! The FUSE callbacks of [`PhoneFs`], each answered by the method of its group.

use std::ffi::OsStr;
use std::time::SystemTime;

use fuser::{
    FileHandle, Filesystem, INodeNo, LockOwner, OpenFlags, RenameFlags, ReplyAttr, ReplyCreate, ReplyData,
    ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyOpen, ReplyStatfs, ReplyWrite, Request, TimeOrNow, WriteFlags,
};

use super::{PhoneFs, TTL};

impl Filesystem for PhoneFs {
    fn destroy(&mut self) {
        // Unmounted while new files were still open: their staging files go with the session.
        self.discard_open_staging();
    }

    fn lookup(&self, _req: &Request, parent_ino: INodeNo, name: &OsStr, reply: ReplyEntry) {
        self.look_up(parent_ino, name, reply);
    }

    fn getattr(&self, _req: &Request, ino: INodeNo, _fh: Option<FileHandle>, reply: ReplyAttr) {
        match self.attr_of(ino) {
            Ok(attr) => reply.attr(&TTL, &attr),
            Err(e) => reply.error(e),
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
        atime: Option<TimeOrNow>,
        mtime: Option<TimeOrNow>,
        _ctime: Option<SystemTime>,
        _fh: Option<FileHandle>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<fuser::BsdFileFlags>,
        reply: ReplyAttr,
    ) {
        self.set_attr(ino, size, atime, mtime, reply);
    }

    fn readdir(&self, _req: &Request, ino: INodeNo, _fh: FileHandle, offset: u64, reply: ReplyDirectory) {
        self.list(ino, offset, reply);
    }

    fn open(&self, req: &Request, ino: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
        self.open_file(ino, flags, req.pid(), reply);
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
        self.read_file(fh, offset, size, reply);
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
        self.write_created(fh, offset, data, reply);
    }

    fn flush(&self, req: &Request, _ino: INodeNo, fh: FileHandle, _lock_owner: LockOwner, reply: ReplyEmpty) {
        self.flush_file(fh, req.pid(), reply);
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
        self.release_handle(fh, reply);
    }

    fn create(
        &self,
        _req: &Request,
        parent_ino: INodeNo,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        _flags: i32,
        reply: ReplyCreate,
    ) {
        self.create_file(parent_ino, name, reply);
    }

    fn mkdir(&self, _req: &Request, parent_ino: INodeNo, name: &OsStr, _mode: u32, _umask: u32, reply: ReplyEntry) {
        self.make_folder(parent_ino, name, reply);
    }

    fn unlink(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        self.unlink_file(parent, name, reply);
    }

    fn rmdir(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        self.remove_folder(parent, name, reply);
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
        self.rename_item(parent, name, newparent, newname, reply);
    }

    fn statfs(&self, _req: &Request, _ino: INodeNo, reply: ReplyStatfs) {
        let state = self.files().state().ok();
        let block = 4096u64;
        let total = state.as_ref().map_or(0, |s| u64::try_from(s.storage_total).unwrap_or_default()) / block;
        let free = state.as_ref().map_or(0, |s| u64::try_from(s.storage_free).unwrap_or_default()) / block;
        reply.statfs(total, free, free, 0, 0, block as u32, 255, block as u32);
    }
}
