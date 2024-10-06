//
// Copyright (c) 2024 Jeff Garzik
//
// This file is part of the posixutils-rs project covered under
// the MIT License.  For the full license text, please see the LICENSE
// file in the root directory of this project.
// SPDX-License-Identifier: MIT
//

#[cfg(target_os = "linux")]
mod mntent;

#[cfg(target_os = "linux")]
use crate::mntent::MountTable;

use clap::Parser;
use gettextrs::{bind_textdomain_codeset, gettext, setlocale, textdomain, LocaleCategory};
use plib::PROJECT_NAME;
#[cfg(target_os = "macos")]
use std::ffi::CStr;
use std::{cmp, ffi::CString, io};

#[derive(Parser)]
#[command(version, about = gettext("df - report free storage space"))]
struct Args {
    #[arg(
        short,
        long,
        help = gettext("Use 1024-byte units, instead of the default 512-byte units, when writing space figures")
    )]
    kilo: bool,

    #[arg(
        short = 'P',
        long,
        help = gettext("Write information in a portable output format")
    )]
    portable: bool,

    #[arg(
        short,
        long,
        help = gettext("Include total allocated-space figures in the output")
    )]
    total: bool,

    #[arg(
        help = gettext("A pathname of a file within the hierarchy of the desired file system")
    )]
    files: Vec<String>,
}

/// Display modes
pub enum OutputMode {
    /// When both the -k and -P options are specified
    Posix,
    /// When the -P option is specified without the -k option
    PosixLegacy,
    /// The format of the default output from df is unspecified,
    /// but all space figures are reported in 512-byte units
    Unspecified,
    Unspecified1K,
}

impl OutputMode {
    pub fn new(kilo: bool, portable: bool) -> Self {
        match (kilo, portable) {
            (true, true) => Self::Posix,
            (true, false) => Self::Unspecified1K,
            (false, true) => Self::PosixLegacy,
            (false, false) => Self::Unspecified,
        }
    }

    pub fn get_block_size(&self) -> u64 {
        match self {
            OutputMode::Posix | OutputMode::Unspecified1K => 1024,
            OutputMode::PosixLegacy | OutputMode::Unspecified => 512,
        }
    }
}

pub struct Field {
    caption: String,
    width: usize,
}

impl Field {
    pub fn new(caption: String, min_width: usize) -> Self {
        let width = cmp::max(caption.len(), min_width);
        Self { caption, width }
    }

    pub fn print_header(&self) {
        print!("{: <width$} ", self.caption, width = self.width);
    }

    pub fn print_header_align_right(&self) {
        print!("{: >width$} ", self.caption, width = self.width);
    }

    pub fn print_string(&self, value: &String) {
        print!("{: <width$} ", value, width = self.width);
    }

    pub fn print_u64(&self, value: u64) {
        print!("{: >width$} ", value, width = self.width);
    }

    pub fn print_percentage(&self, value: u32) {
        print!("{: >width$}% ", value, width = self.width - 1);
    }
}

pub struct Fields {
    pub mode: OutputMode,
    /// file system
    pub source: Field,
    /// FS size
    pub size: Field,
    /// FS size used
    pub used: Field,
    /// FS size available
    pub avail: Field,
    /// percent used
    pub pcent: Field,
    /// mount point
    pub target: Field,
    // /// specified file name
    // file: Field,
}

impl Fields {
    pub fn new(mode: OutputMode) -> Self {
        let size_caption = format!("{}-{}", mode.get_block_size(), gettext("blocks"));
        Self {
            mode,
            source: Field::new(gettext("Filesystem"), 14),
            size: Field::new(size_caption, 10),
            used: Field::new(gettext("Used"), 10),
            avail: Field::new(gettext("Available"), 10),
            pcent: Field::new(gettext("Capacity"), 5),
            target: Field::new(gettext("Mounted on"), 0),
        }
    }

    pub fn print_header(&self) {
        self.source.print_header();
        self.size.print_header_align_right();
        self.used.print_header_align_right();
        self.avail.print_header_align_right();
        self.pcent.print_header_align_right();
        self.target.print_header();
        println!();
    }

    fn print_row(
        &self,
        fsname: &String,
        total: u64,
        used: u64,
        avail: u64,
        percentage_used: u32,
        target: &String,
    ) {
        // The remaining output with -P shall consist of one line of information
        // for each specified file system. These lines shall be formatted as follows:
        // "%s %d %d %d %d%% %s\n", <file system name>, <total space>,
        //     <space used>, <space free>, <percentage used>,
        //     <file system root>
        self.source.print_string(fsname);
        self.size.print_u64(total);
        self.used.print_u64(used);
        self.avail.print_u64(avail);
        self.pcent.print_percentage(percentage_used);
        self.target.print_string(target);
        println!();
    }
}

#[cfg(target_os = "macos")]
fn to_cstr(array: &[libc::c_char]) -> &CStr {
    unsafe {
        // Assuming the array is null-terminated, as it should be for C strings.
        CStr::from_ptr(array.as_ptr())
    }
}

fn stat(filename: &CString) -> io::Result<libc::stat> {
    unsafe {
        let mut st: libc::stat = std::mem::zeroed();
        let rc = libc::stat(filename.as_ptr(), &mut st);
        if rc == 0 {
            Ok(st)
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

struct Mount {
    devname: String,
    dir: String,
    dev: i64,
    masked: bool,
    cached_statfs: libc::statfs,
}

impl Mount {
    fn print(&self, fields: &Fields) {
        if !self.masked {
            return;
        }

        let sf = self.cached_statfs;

        let block_size = fields.mode.get_block_size();
        let blksz = sf.f_bsize as u64;

        let total = (sf.f_blocks * blksz) / block_size;
        let avail = (sf.f_bavail * blksz) / block_size;
        let free = (sf.f_bfree * blksz) / block_size;
        let used = total - free;

        // The percentage value shall be expressed as a positive integer,
        // with any fractional result causing it to be rounded to the next highest integer.
        let percentage_used = f64::from(used as u32) / f64::from((used + free) as u32);
        let percentage_used = percentage_used * 100.0;
        let percentage_used = percentage_used.ceil() as u32;

        fields.print_row(
            &self.devname,
            total,
            used,
            avail,
            percentage_used,
            &self.dir,
        );
    }
}

struct MountList {
    mounts: Vec<Mount>,
    has_masks: bool,
}

impl MountList {
    fn new() -> MountList {
        MountList {
            mounts: Vec::new(),
            has_masks: false,
        }
    }

    fn mask_all(&mut self) {
        for mount in &mut self.mounts {
            mount.masked = true;
        }
    }

    fn ensure_masked(&mut self) {
        if !self.has_masks {
            self.mask_all();
            self.has_masks = true;
        }
    }

    fn push(&mut self, fsstat: &libc::statfs, devname: &CString, dirname: &CString) {
        let dev = {
            if let Ok(st) = stat(devname) {
                st.st_rdev as i64
            } else if let Ok(st) = stat(dirname) {
                st.st_dev as i64
            } else {
                -1
            }
        };

        self.mounts.push(Mount {
            devname: String::from(devname.to_str().unwrap()),
            dir: String::from(dirname.to_str().unwrap()),
            dev,
            masked: false,
            cached_statfs: *fsstat,
        });
    }
}

#[cfg(target_os = "macos")]
fn read_mount_info() -> io::Result<MountList> {
    let mut info = MountList::new();

    unsafe {
        let mut mounts: *mut libc::statfs = std::ptr::null_mut();
        let n_mnt = libc::getmntinfo(&mut mounts, libc::MNT_WAIT);
        if n_mnt < 0 {
            return Err(io::Error::last_os_error());
        }

        let mounts: &[libc::statfs] = std::slice::from_raw_parts(mounts as _, n_mnt as _);
        for mount in mounts {
            let devname = to_cstr(&mount.f_mntfromname).into();
            let dirname = to_cstr(&mount.f_mntonname).into();
            info.push(mount, &devname, &dirname);
        }
    }

    Ok(info)
}

#[cfg(target_os = "linux")]
fn read_mount_info() -> io::Result<MountList> {
    let mut info = MountList::new();

    let mounts = MountTable::try_new()?;
    for mount in mounts {
        unsafe {
            let mut buf: libc::statfs = std::mem::zeroed();
            let rc = libc::statfs(mount.dir.as_ptr(), &mut buf);
            if rc < 0 {
                eprintln!(
                    "{}: {}",
                    mount.dir.to_str().unwrap(),
                    io::Error::last_os_error()
                );
                continue;
            }

            info.push(&buf, &mount.fsname, &mount.dir);
        }
    }

    Ok(info)
}

fn mask_fs_by_file(info: &mut MountList, filename: &str) -> io::Result<()> {
    let c_filename = CString::new(filename).expect("`filename` contains an internal 0 byte");
    let stat_res = stat(&c_filename);
    if let Err(e) = stat_res {
        eprintln!("{}: {}", filename, e);
        return Err(e);
    }
    let stat = stat_res.unwrap();

    for mount in &mut info.mounts {
        if stat.st_dev as i64 == mount.dev {
            info.has_masks = true;
            mount.masked = true;
        }
    }

    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // parse command line arguments
    let args = Args::parse();

    setlocale(LocaleCategory::LcAll, "");
    textdomain(PROJECT_NAME)?;
    bind_textdomain_codeset(PROJECT_NAME, "UTF-8")?;

    let mut info = read_mount_info()?;

    if args.files.is_empty() {
        info.mask_all();
    } else {
        for file in &args.files {
            mask_fs_by_file(&mut info, file)?;
        }
    }

    info.ensure_masked();

    let mode = OutputMode::new(args.kilo, args.portable);
    let fields = Fields::new(mode);
    fields.print_header();

    for mount in &info.mounts {
        mount.print(&fields);
    }

    Ok(())
}
