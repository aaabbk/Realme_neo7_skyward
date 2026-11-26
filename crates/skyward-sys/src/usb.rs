//! The functions the USB port offers, which init sets up as
//! `sys.usb.config` asks: adb while the recovery waits, with files over MTP
//! beside it while file transfer is on.
//!
//! A port the kernel bound to a controller keeps the functions it was bound
//! with: another set of them needs the port taken off the controller first,
//! which this kernel does for a blank line and for no name at all. What the
//! port is doing is read from the kernel rather than from the properties
//! init keeps, which say what init was asked for and not what the kernel
//! took.

use crate::{kmsg, prop, set_prop, wait_prop};
use anyhow::{ensure, Context, Result};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

static MTP: AtomicBool = AtomicBool::new(false);

/// What the port offers while nothing else has it.
pub fn resting() -> &'static str {
    if MTP.load(Ordering::Relaxed) {
        "mtp,adb"
    } else {
        "adb"
    }
}

/// Whether MTP goes beside adb while the port rests: from when its function
/// has its descriptors until it goes.
pub fn set_mtp(on: bool) {
    MTP.store(on, Ordering::Relaxed);
}

/// Where the kernel keeps the controllers it made, where the gadget made
/// from configfs keeps what it is bound to, and the configuration whose
/// functions are the port's.
const CONTROLLERS: &str = "/sys/class/udc";
const GADGET_UDC: &str = "/config/usb_gadget/g1/UDC";
const GADGET_CONFIG: &str = "/config/usb_gadget/g1/configs/b.1";

/// Where the gadget the kernel has always had keeps what it is doing.
const OLDER: &str = "/sys/class/android_usb/android0";

/// Whether the port is in place for `config`: the gadget the phone's rules
/// chose bound to a controller with each function `config` names on it, or
/// the older gadget enabled with them.
pub fn in_place(config: &str) -> bool {
    let named = |f: &str| format!("ffs.{f}");
    match prop("sys.usb.configfs").as_deref() {
        Some("0") => {
            let read = |name: &str| std::fs::read_to_string(format!("{OLDER}/{name}")).map(|s| s.trim().to_string()).unwrap_or_default();
            let functions = read("functions");
            read("enable") == "1" && config.split(',').all(|f| functions.split(',').any(|has| has == f))
        }
        _ => {
            let bound = std::fs::read_to_string(GADGET_UDC).unwrap_or_default();
            !bound.trim().is_empty() && config.split(',').all(|f| Path::new(&format!("{GADGET_CONFIG}/{}", named(f))).exists())
        }
    }
}

/// What the port is doing now, for the log: what init was asked for, what it
/// says it put in place, and what the kernel has bound.
pub fn describe() -> String {
    let asked = |name: &str| prop(name).unwrap_or_else(|| "unset".into());
    let read = |path: &str| std::fs::read_to_string(path).map(|said| said.trim().to_string()).unwrap_or_default();
    let controller = asked("sys.usb.controller");
    let bound = read(GADGET_UDC);
    let port = read(&format!("{CONTROLLERS}/{controller}/state"));
    let older = read(&format!("{OLDER}/enable"));
    format!(
        "asked {}, said {}, configfs {}, controller {controller}, adbd {}, file transfer {}, bound to {}, port {}, older gadget enable {}",
        asked("sys.usb.config"),
        asked("sys.usb.state"),
        asked("sys.usb.configfs"),
        asked("init.svc.adbd"),
        asked("sys.usb.ffs.ready"),
        if bound.is_empty() { "nothing" } else { bound.as_str() },
        if port.is_empty() { "unread" } else { port.as_str() },
        if older.is_empty() { "unread" } else { older.as_str() },
    )
}

/// Bind the gadget to a controller the kernel made: the one the phone's own
/// rules name where the kernel has it, and otherwise the first of them.
pub fn bind() -> Result<String> {
    let mut names: Vec<String> = std::fs::read_dir(CONTROLLERS)
        .with_context(|| format!("read {CONTROLLERS}"))?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let named = prop("sys.usb.controller").filter(|name| names.contains(name));
    let name = named.or_else(|| names.into_iter().next()).with_context(|| format!("{CONTROLLERS} holds no controller"))?;
    let bound = std::fs::read_to_string(GADGET_UDC).unwrap_or_default();
    if bound.trim() != name {
        std::fs::write(GADGET_UDC, &name).with_context(|| format!("bind {GADGET_UDC} to {name}"))?;
    }
    Ok(name)
}

/// Take the port off the controller, so the functions on it can be changed,
/// and so a computer sees it go. The kernel takes no name for nothing, but
/// a blank line takes the port off.
pub fn unbind() -> Result<()> {
    if std::fs::read_to_string(GADGET_UDC).is_ok_and(|bound| bound.trim().is_empty()) {
        return Ok(());
    }
    std::fs::write(GADGET_UDC, "\n").with_context(|| format!("take the port off {GADGET_UDC}"))
}

/// Ask init for `config` and see it through, binding the port here where
/// init could not: a port named to a controller the kernel does not have is
/// never in place, and the phone is then reached by no computer at all.
pub fn ensure(config: &str, timeout: Duration) -> Result<String> {
    set_prop("sys.usb.config", config)?;
    if wait_for(config, timeout).is_ok() {
        return Ok(String::new());
    }
    let before = describe();
    // The function has to be there before the port is bound, as init binds it.
    let ours = (|| -> Result<String> {
        if prop("sys.usb.ffs.ready").as_deref() != Some("1") {
            wait_prop("sys.usb.ffs.ready", "1", timeout)?;
        }
        let controller = bind()?;
        wait_for(config, timeout)?;
        Ok(controller)
    })();
    match ours {
        Ok(controller) => Ok(controller),
        // The gadget this kernel builds from configfs does not take the
        // port: the one it has always had may, and the phone's own recovery
        // reaches a computer through it.
        Err(e) => {
            let _ = kmsg(&format!("the port did not come up: {before}; binding it here did not either: {e:#}"));
            if prop("sys.usb.configfs").as_deref() == Some("0") {
                return Err(e);
            }
            set_prop("sys.usb.configfs", "0")?;
            set_prop("sys.usb.config", "none")?;
            thread::sleep(Duration::from_millis(200));
            set_prop("sys.usb.config", config)?;
            wait_for(config, timeout)?;
            let _ = kmsg(&format!("the port came up on the gadget the kernel has always had: {}", describe()));
            Ok("the older gadget".to_string())
        }
    }
}

/// Wait until the port is in place for `config`, as the kernel tells it.
fn wait_for(config: &str, timeout: Duration) -> Result<()> {
    let start = Instant::now();
    loop {
        if in_place(config) {
            return Ok(());
        }
        ensure!(
            start.elapsed() < timeout,
            "the port is not in place for {config} after {} ms: {}",
            timeout.as_millis(),
            describe()
        );
        thread::sleep(Duration::from_millis(20));
    }
}

/// Ask init for `config` and wait until it is in place. Going to "none"
/// takes the port off the controller and waits for adbd to be gone, so its
/// function is free for the next configuration.
pub fn set(config: &str, timeout: Duration) -> Result<()> {
    if config != "none" {
        set_prop("sys.usb.config", config)?;
        return wait_for(config, timeout);
    }
    unbind()?;
    set_prop("sys.usb.config", "none")?;
    let start = Instant::now();
    while !matches!(prop("init.svc.adbd").as_deref(), None | Some("stopped")) {
        ensure!(start.elapsed() < timeout, "adbd still runs {} ms after the port went to none", timeout.as_millis());
        thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

/// Hand the port to `config` through "none", which stops the programs
/// serving it and takes every function off; init then sets the port up
/// once the programs behind the new configuration are ready. A port already
/// in place is left as it is.
pub fn switch(config: &str, timeout: Duration) -> Result<()> {
    if in_place(config) {
        return Ok(());
    }
    set("none", timeout)?;
    set(config, timeout)
}
