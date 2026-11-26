//! The functions the USB port offers, which init sets up as
//! `sys.usb.config` asks: adb while the recovery waits, with files over MTP
//! beside it while file transfer is on.

use crate::{prop, set_prop, wait_prop};
use anyhow::{ensure, Context, Result};
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

/// Where the kernel keeps the controllers it made, and where the gadget
/// takes the one it binds to.
const CONTROLLERS: &str = "/sys/class/udc";
const GADGET_UDC: &str = "/config/usb_gadget/g1/UDC";

/// Bind the gadget to a controller the kernel made. A phone's own rules name
/// one in a property; a recovery of our own may not set it, or may name one
/// this kernel does not have, so the kernel's list is read and the first
/// controller written where the gadget takes one.
pub fn bind() -> Result<String> {
    let mut names: Vec<String> = std::fs::read_dir(CONTROLLERS)
        .with_context(|| format!("read {CONTROLLERS}"))?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let name = names.into_iter().next().with_context(|| format!("{CONTROLLERS} holds no controller"))?;
    let bound = std::fs::read_to_string(GADGET_UDC).unwrap_or_default();
    if bound.trim() != name {
        std::fs::write(GADGET_UDC, &name).with_context(|| format!("write {GADGET_UDC}"))?;
    }
    Ok(name)
}

/// Ask init for `config` and see it through, binding the port here where
/// init could not: a port named to a controller the kernel does not have is
/// never in place, and the phone is then reached by no computer at all.
pub fn ensure(config: &str, timeout: Duration) -> Result<String> {
    set_prop("sys.usb.config", config)?;
    if wait_prop("sys.usb.state", config, timeout).is_ok() {
        return Ok(String::new());
    }
    // The function has to be there before the port is bound, as init binds it.
    if prop("sys.usb.ffs.ready").as_deref() != Some("1") {
        wait_prop("sys.usb.ffs.ready", "1", timeout)?;
    }
    let controller = bind()?;
    wait_prop("sys.usb.state", config, timeout)?;
    Ok(controller)
}

/// Ask init for `config` and wait until it is in place. Going to "none"
/// also waits for adbd to be gone, so its function is free.
pub fn set(config: &str, timeout: Duration) -> Result<()> {
    set_prop("sys.usb.config", config)?;
    wait_prop("sys.usb.state", config, timeout)?;
    if config == "none" {
        let start = Instant::now();
        while !matches!(prop("init.svc.adbd").as_deref(), None | Some("stopped")) {
            ensure!(start.elapsed() < timeout, "adbd still runs {} ms after the port went to none", timeout.as_millis());
            thread::sleep(Duration::from_millis(10));
        }
    }
    Ok(())
}

/// Hand the port to `config` through "none", which stops the programs
/// serving it and takes every function off; init then sets the port up
/// once the programs behind the new configuration are ready.
pub fn switch(config: &str, timeout: Duration) -> Result<()> {
    set("none", timeout)?;
    set(config, timeout)
}
