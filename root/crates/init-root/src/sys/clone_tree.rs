//! Linux snapshot copy using pinned directory descriptors. Never traverse source links.
//! The caller owns the destination parent and discards any partial destination on failure.
//! Root system snapshots are crash-consistent: they may race live writers, so this walker
//! refuses a source whose identity/metadata changes while copied rather than accepting a
//! partial or mixed file. Callers needing transaction consistency must freeze writers.
use std::ffi::{CStr, CString};
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path};

const MAX_DEPTH: usize = 128;
const MAX_ENTRIES: usize = 1_000_000;
fn unsupported(message: &str) -> io::Error { io::Error::new(io::ErrorKind::Unsupported,message) }
fn changed() -> io::Error { io::Error::new(io::ErrorKind::InvalidData,"snapshot source changed during copy") }
fn cstr(bytes: &[u8]) -> io::Result<CString> { CString::new(bytes).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput,"NUL in snapshot path")) }
fn openat(dir: RawFd, name: &CStr, flags: i32, mode: libc::mode_t) -> io::Result<OwnedFd> {
    // SAFETY: name is terminated; the returned descriptor is checked and has unique ownership.
    let fd=unsafe { libc::openat(dir,name.as_ptr(),flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,mode) };
    if fd<0 { Err(io::Error::last_os_error()) } else { Ok(unsafe { OwnedFd::from_raw_fd(fd) }) }
}
fn statat(dir: RawFd, name: &CStr) -> io::Result<libc::stat> {
    let mut stat=std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: stat is writable and initialized only when fstatat succeeds; links are not followed.
    if unsafe { libc::fstatat(dir,name.as_ptr(),stat.as_mut_ptr(),libc::AT_SYMLINK_NOFOLLOW) } != 0 { return Err(io::Error::last_os_error()); }
    Ok(unsafe { stat.assume_init() })
}
fn statfd(fd: RawFd) -> io::Result<libc::stat> {
    let mut stat=std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: fstat initializes the valid output pointer on success.
    if unsafe { libc::fstat(fd,stat.as_mut_ptr()) } != 0 { return Err(io::Error::last_os_error()); }
    Ok(unsafe { stat.assume_init() })
}
fn same(a:&libc::stat,b:&libc::stat) -> bool {
    a.st_dev==b.st_dev && a.st_ino==b.st_ino && a.st_mode==b.st_mode && a.st_size==b.st_size
        && a.st_mtime==b.st_mtime && a.st_mtime_nsec==b.st_mtime_nsec
        && a.st_ctime==b.st_ctime && a.st_ctime_nsec==b.st_ctime_nsec
}
/// Open every ancestor without following links, pinning the final parent before touching its leaf.
fn parent(path:&Path) -> io::Result<(OwnedFd,CString)> {
    let mut fd=openat(libc::AT_FDCWD,if path.is_absolute() { c"/" } else { c"." },libc::O_RDONLY | libc::O_DIRECTORY,0)?;
    let mut parts=Vec::new();
    for component in path.components() {
        match component {
            Component::RootDir | Component::CurDir => {},
            Component::Normal(name) => parts.push(cstr(name.as_bytes())?),
            _ => return Err(io::Error::new(io::ErrorKind::InvalidInput,"snapshot paths cannot contain parent traversal")),
        }
    }
    let leaf=parts.pop().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput,"snapshot path needs a leaf"))?;
    for part in parts {
        let before=statat(fd.as_raw_fd(),&part)?;
        if before.st_mode & libc::S_IFMT != libc::S_IFDIR { return Err(unsupported("snapshot ancestor must be a real directory")); }
        let next=openat(fd.as_raw_fd(),&part,libc::O_RDONLY | libc::O_DIRECTORY,0)?;
        let after=statfd(next.as_raw_fd())?;
        if before.st_dev!=after.st_dev || before.st_ino!=after.st_ino || after.st_mode & libc::S_IFMT != libc::S_IFDIR { return Err(changed()); }
        fd=next;
    }
    Ok((fd,leaf))
}
fn no_xattrs(fd:RawFd) -> io::Result<()> {
    // Dropping a POSIX ACL could broaden access on restore. Refuse metadata we cannot preserve.
    // SAFETY: null with length zero requests the required buffer length only.
    let n=unsafe { libc::flistxattr(fd,std::ptr::null_mut(),0) };
    if n>0 { return Err(unsupported("Linux snapshot copy cannot preserve extended attributes/ACLs")); }
    if n<0 {
        let e=io::Error::last_os_error();
        if !matches!(e.raw_os_error(),Some(libc::ENOTSUP)) { return Err(e); }
    }
    Ok(())
}
fn metadata(fd:RawFd,stat:&libc::stat) -> io::Result<()> {
    // No ownership copying (same semantics as CLONE_NOOWNERCOPY), nor new set-id privileges.
    // SAFETY: fd is a held regular-file/directory descriptor; times is a valid 2-element array.
    if unsafe { libc::fchmod(fd,stat.st_mode & 0o1777) } != 0 { return Err(io::Error::last_os_error()); }
    let times=[libc::timespec { tv_sec:stat.st_atime,tv_nsec:stat.st_atime_nsec },libc::timespec { tv_sec:stat.st_mtime,tv_nsec:stat.st_mtime_nsec }];
    if unsafe { libc::futimens(fd,times.as_ptr()) } != 0 { return Err(io::Error::last_os_error()); }
    if unsafe { libc::fsync(fd) } != 0 { return Err(io::Error::last_os_error()); }
    Ok(())
}
struct Dir(*mut libc::DIR);
impl Drop for Dir { fn drop(&mut self) { /* SAFETY: this stream uniquely owns its duplicated fd. */ unsafe { libc::closedir(self.0); } } }
fn entries(fd:RawFd) -> io::Result<Vec<CString>> {
    // SAFETY: duplicate a held fd with CLOEXEC; fdopendir owns it only after success.
    let duplicate=unsafe { libc::fcntl(fd,libc::F_DUPFD_CLOEXEC,3) };
    if duplicate<0 { return Err(io::Error::last_os_error()); }
    let duplicate=unsafe { OwnedFd::from_raw_fd(duplicate) };
    let stream=unsafe { libc::fdopendir(duplicate.as_raw_fd()) };
    if stream.is_null() { return Err(io::Error::last_os_error()); }
    let _=duplicate.into_raw_fd(); let stream=Dir(stream); let mut names=Vec::new();
    loop {
        // SAFETY: this thread owns stream; reset errno to distinguish EOF from read failure.
        unsafe { *libc::__errno_location()=0; }
        let entry=unsafe { libc::readdir(stream.0) };
        if entry.is_null() {
            let e=io::Error::last_os_error(); if e.raw_os_error()!=Some(0) { return Err(e); } break;
        }
        // SAFETY: readdir's d_name is terminated, valid until the next call; copy it now.
        let name=unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        if name.to_bytes()==b"." || name.to_bytes()==b".." { continue; }
        if names.len()>=MAX_ENTRIES { return Err(unsupported("snapshot has too many directory entries")); }
        names.push(name.to_owned());
    }
    names.sort_by(|a,b| a.to_bytes().cmp(b.to_bytes())); Ok(names)
}
struct Budget { entries:usize, bytes:u64, max_bytes:u64 }
impl Budget {
    fn reserve(&mut self,bytes:u64) -> io::Result<()> {
        self.bytes=self.bytes.checked_add(bytes).filter(|total| *total<=self.max_bytes)
            .ok_or_else(|| io::Error::new(io::ErrorKind::FileTooLarge,"snapshot logical-byte quota exceeded"))?;
        Ok(())
    }
}
fn copy(src:RawFd,name:&CStr,dst:RawFd,to:&CStr,depth:usize,budget:&mut Budget,skip:&[&str]) -> io::Result<()> {
    if depth>MAX_DEPTH || budget.entries>=MAX_ENTRIES { return Err(unsupported("snapshot tree exceeds traversal limits")); }
    budget.entries+=1;
    let before=statat(src,name)?;
    if before.st_mode & 0o6000 != 0 { return Err(unsupported("snapshot refuses set-id metadata without ownership copying")); }
    match before.st_mode & libc::S_IFMT {
        libc::S_IFDIR => {
            let source=openat(src,name,libc::O_RDONLY | libc::O_DIRECTORY,0)?;
            if !same(&before,&statfd(source.as_raw_fd())?) { return Err(changed()); }
            no_xattrs(source.as_raw_fd())?;
            // SAFETY: pinned parent and terminated destination name; mkdirat refuses existing entries.
            if unsafe { libc::mkdirat(dst,to.as_ptr(),0o700) } != 0 { return Err(io::Error::last_os_error()); }
            let dest=openat(dst,to,libc::O_RDONLY | libc::O_DIRECTORY,0)?;
            for child in entries(source.as_raw_fd())? {
                if depth==0 && skip.iter().any(|entry| child.to_bytes()==entry.as_bytes()) { continue; }
                copy(source.as_raw_fd(),&child,dest.as_raw_fd(),&child,depth+1,budget,&[])?;
            }
            if !same(&before,&statfd(source.as_raw_fd())?) { return Err(changed()); }
            metadata(dest.as_raw_fd(),&before)?;
        }
        libc::S_IFREG => {
            let source=openat(src,name,libc::O_RDONLY | libc::O_NONBLOCK,0)?;
            if !same(&before,&statfd(source.as_raw_fd())?) { return Err(changed()); }
            no_xattrs(source.as_raw_fd())?;
            if before.st_size<0 { return Err(changed()); }
            let bytes=before.st_size as u64;
            budget.reserve(bytes)?; // Before FICLONE or the first copied byte.
            let dest=openat(dst,to,libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NONBLOCK,0o600)?;
            // SAFETY: FICLONE takes a held source fd; it does not follow either pathname.
            const FICLONE:libc::c_ulong=0x40049409;
            let cloned=unsafe { libc::ioctl(dest.as_raw_fd(),FICLONE,source.as_raw_fd()) }==0;
            if !cloned {
                let e=io::Error::last_os_error();
                if !matches!(e.raw_os_error(),Some(libc::EOPNOTSUPP | libc::EXDEV | libc::EINVAL | libc::ENOTTY)) { return Err(e); }
                let mut source=File::from(source); let mut dest=File::from(dest);
                let copied=io::copy(&mut (&mut source).take(before.st_size as u64+1),&mut dest)?;
                if copied!=before.st_size as u64 || !same(&before,&statfd(source.as_raw_fd())?) { return Err(changed()); }
                metadata(dest.as_raw_fd(),&before)?;
            } else {
                if !same(&before,&statfd(source.as_raw_fd())?) { return Err(changed()); }
                metadata(dest.as_raw_fd(),&before)?;
            }
        }
        libc::S_IFLNK => {
            let mut buf=vec![0u8;65536];
            // SAFETY: readlinkat copies at most the buffer length and never follows this link.
            let n=unsafe { libc::readlinkat(src,name.as_ptr(),buf.as_mut_ptr().cast(),buf.len()) };
            if n<0 { return Err(io::Error::last_os_error()); }
            if n as usize==buf.len() { return Err(unsupported("snapshot link target too long")); }
            budget.reserve(n as u64)?; // Long link contents consume disk too.
            let target=cstr(&buf[..n as usize])?;
            if !same(&before,&statat(src,name)?) { return Err(changed()); }
            // SAFETY: both strings are terminated; target is copied verbatim, never resolved.
            if unsafe { libc::symlinkat(target.as_ptr(),dst,to.as_ptr()) } != 0 { return Err(io::Error::last_os_error()); }
            let times=[libc::timespec { tv_sec:before.st_atime,tv_nsec:before.st_atime_nsec },libc::timespec { tv_sec:before.st_mtime,tv_nsec:before.st_mtime_nsec }];
            if unsafe { libc::utimensat(dst,to.as_ptr(),times.as_ptr(),libc::AT_SYMLINK_NOFOLLOW) } != 0 { return Err(io::Error::last_os_error()); }
        }
        _ => return Err(unsupported("snapshot refuses sockets, FIFOs and devices")),
    }
    Ok(())
}
pub(super) fn clonefile_bounded(src:&Path,dst:&Path,max_bytes:u64,skip:&[&str]) -> io::Result<u64> {
    let (source,name)=parent(src)?; let (dest,to)=parent(dst)?;
    // A copy into its own source could grow indefinitely. Pinned tree traversal below is bounded,
    // and this rejects the common accidental overlap before creating any output.
    if dst.starts_with(src) { return Err(io::Error::new(io::ErrorKind::InvalidInput,"snapshot destination is inside source")); }
    let mut budget=Budget { entries:0,bytes:0,max_bytes };
    copy(source.as_raw_fd(),&name,dest.as_raw_fd(),&to,0,&mut budget,skip)?;
    // SAFETY: fsync the pinned destination directory so its new entry is durable.
    if unsafe { libc::fsync(dest.as_raw_fd()) } != 0 { return Err(io::Error::last_os_error()); }
    Ok(budget.bytes)
}
pub(super) fn clonefile(src:&Path,dst:&Path) -> io::Result<()> {
    clonefile_bounded(src,dst,u64::MAX,&[]).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink,PermissionsExt};
    fn home(name:&str) -> std::path::PathBuf {
        let base=Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-homes/clone-tree");
        std::fs::create_dir_all(&base).unwrap();
        let path=base.canonicalize().unwrap().join(format!("{name}-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir(&path).unwrap(); path
    }
    #[test]
    fn clone_tree_preserves_bytes_modes_and_links_without_following() {
        let h=home("tree"); let src=h.join("source"); let dst=h.join("copy");
        std::fs::create_dir_all(src.join("nested")).unwrap();
        std::fs::write(src.join("nested/file"),b"original").unwrap();
        std::fs::set_permissions(src.join("nested/file"),std::fs::Permissions::from_mode(0o751)).unwrap();
        std::fs::write(h.join("outside"),b"secret outside tree").unwrap();
        symlink("../../outside",src.join("nested/link")).unwrap();
        symlink("missing",src.join("broken")).unwrap();
        clonefile(&src,&dst).unwrap();
        assert_eq!(std::fs::read(dst.join("nested/file")).unwrap(),b"original");
        assert_eq!(std::fs::metadata(dst.join("nested/file")).unwrap().permissions().mode() & 0o777,0o751);
        assert_eq!(std::fs::read_link(dst.join("nested/link")).unwrap(),Path::new("../../outside"));
        assert!(std::fs::symlink_metadata(dst.join("broken")).unwrap().file_type().is_symlink());
        std::fs::write(dst.join("nested/file"),b"modified").unwrap();
        assert_eq!(std::fs::read(src.join("nested/file")).unwrap(),b"original");
        assert!(clonefile(&src,&dst).is_err());
        clonefile(&src.join("broken"),&h.join("link-copy")).unwrap();
        assert_eq!(std::fs::read_link(h.join("link-copy")).unwrap(),Path::new("missing"));
        std::fs::remove_dir_all(h).unwrap();
    }
    #[test]
    fn clone_tree_refuses_ancestor_links_fifo_socket_and_existing_destination() {
        let h=home("refuse"); std::fs::create_dir(h.join("real")).unwrap();
        std::fs::write(h.join("real/file"),b"data").unwrap();
        symlink("real",h.join("alias")).unwrap();
        assert!(clonefile(&h.join("alias/file"),&h.join("copy")).is_err());
        assert!(clonefile(&h.join("real/file"),&h.join("alias/copy")).is_err());
        assert!(!h.join("real/copy").exists());
        let fifo=cstr(h.join("fifo").as_os_str().as_bytes()).unwrap();
        // SAFETY: creates only this test's owned FIFO pathname; no reader/writer is spawned.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(),0o600) },0);
        let started=std::time::Instant::now();
        assert_eq!(clonefile(&h.join("fifo"),&h.join("fifo-copy")).unwrap_err().kind(),io::ErrorKind::Unsupported);
        assert!(started.elapsed()<std::time::Duration::from_secs(2));
        let listener=std::os::unix::net::UnixListener::bind(h.join("socket")).unwrap();
        assert_eq!(clonefile(&h.join("socket"),&h.join("socket-copy")).unwrap_err().kind(),io::ErrorKind::Unsupported);
        drop(listener);
        assert!(clonefile(&h.join("real"),&h.join("real/inside")).is_err());
        std::fs::remove_dir_all(h).unwrap();
    }
    #[test]
    fn clone_tree_refuses_unpreserved_extended_metadata() {
        let h=home("xattrs"); let src=h.join("file"); std::fs::write(&src,b"data").unwrap();
        let path=cstr(src.as_os_str().as_bytes()).unwrap(); let value=b"metadata";
        // SAFETY: valid terminated path/name and fixed value buffer, owned by this test.
        let rc=unsafe { libc::setxattr(path.as_ptr(),c"user.init-clone-test".as_ptr(),value.as_ptr().cast(),value.len(),0) };
        if rc==0 { assert_eq!(clonefile(&src,&h.join("copy")).unwrap_err().kind(),io::ErrorKind::Unsupported); }
        else { assert!(matches!(io::Error::last_os_error().raw_os_error(),Some(libc::ENOTSUP | libc::EPERM))); }
        std::fs::remove_dir_all(h).unwrap();
    }
    #[test]
    fn clone_tree_bounded_quota_precedes_copy_and_excluded_trees_are_not_opened() {
        let h=home("bounded"); let src=h.join("source");
        std::fs::create_dir_all(src.join("sessions")).unwrap();
        std::fs::create_dir(src.join("nested")).unwrap();
        std::fs::write(src.join("a"),b"ab").unwrap();
        std::fs::write(src.join("nested/b"),b"c").unwrap();
        // An excluded conversation may be huge and contain unsupported objects;
        // neither its bytes nor its children belong to the system snapshot.
        File::create(src.join("sessions/large")).unwrap().set_len(1<<30).unwrap();
        let fifo=cstr(src.join("sessions/fifo").as_os_str().as_bytes()).unwrap();
        // SAFETY: creates only this test's private excluded FIFO, never opens it.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(),0o600) },0);
        assert_eq!(clonefile_bounded(&src,&h.join("exact"),3,&["sessions"]).unwrap(),3);
        assert_eq!(std::fs::read(h.join("exact/a")).unwrap(),b"ab");
        assert!(!h.join("exact/sessions").exists());
        let error=clonefile_bounded(&src,&h.join("too-small"),1,&["sessions"]).unwrap_err();
        assert_eq!(error.kind(),io::ErrorKind::FileTooLarge);
        assert!(!h.join("too-small/a").exists(),"quota checked before creating/copying the file");
        assert_eq!(std::fs::read(src.join("a")).unwrap(),b"ab");
        std::fs::remove_dir_all(h).unwrap();
    }
}
