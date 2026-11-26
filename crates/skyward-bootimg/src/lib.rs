//! Android boot images with header version 3 or 4, the `vendor_boot` images
//! whose ramdisk comes in fragments, and the newc archives their ramdisks
//! are: read, changed and written again, by the image packer on a computer
//! and by the recovery on the phone.

mod image;
mod ramdisk;
mod vendor;

pub use image::BootImage;
pub use ramdisk::{lz4_legacy, Entry, Ramdisk};
pub use vendor::{unpack_legacy, Fragment, VendorBoot, DLKM, PLATFORM, RECOVERY};
