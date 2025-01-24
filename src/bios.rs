use anyhow::{bail, Result};
use std::io::prelude::*;
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::process::Command;

use crate::blockdev;
use crate::component::*;
use crate::model::*;
use crate::packagesystem;

// grub2-install file path
pub(crate) const GRUB_BIN: &str = "usr/sbin/grub2-install";

#[derive(Default)]
pub(crate) struct Bios {}

impl Bios {
    // Return `true` if grub2-modules installed
    fn check_grub_modules(&self) -> Result<bool> {
        let usr_path = Path::new("/usr/lib/grub");
        #[cfg(target_arch = "x86_64")]
        {
            usr_path.join("i386-pc").try_exists().map_err(Into::into)
        }
        #[cfg(target_arch = "powerpc64")]
        {
            usr_path
                .join("powerpc-ieee1275")
                .try_exists()
                .map_err(Into::into)
        }
    }

    // Run grub2-install
    fn run_grub_install(&self, dest_root: &str, device: &str) -> Result<()> {
        if !self.check_grub_modules()? {
            bail!("Failed to find grub2-modules");
        }
        let grub_install = Path::new("/").join(GRUB_BIN);
        if !grub_install.exists() {
            bail!("Failed to find {:?}", grub_install);
        }

        let mut cmd = Command::new(grub_install);
        let boot_dir = Path::new(dest_root).join("boot");
        // We forcibly inject mdraid1x because it's needed by CoreOS's default of "install raw disk image"
        // We also add part_gpt because in some cases probing of the partition map can fail such
        // as in a container, but we always use GPT.
        #[cfg(target_arch = "x86_64")]
        cmd.args(["--target", "i386-pc"])
            .args(["--boot-directory", boot_dir.to_str().unwrap()])
            .args(["--modules", "mdraid1x part_gpt"])
            .arg(device);

        #[cfg(target_arch = "powerpc64")]
        cmd.args(&["--target", "powerpc-ieee1275"])
            .args(&["--boot-directory", boot_dir.to_str().unwrap()])
            .arg("--no-nvram")
            .arg(device);

        let cmdout = cmd.output()?;
        if !cmdout.status.success() {
            std::io::stderr().write_all(&cmdout.stderr)?;
            bail!("Failed to run {:?}", cmd);
        }
        Ok(())
    }

    // check bios_boot partition on gpt type disk
    fn get_bios_boot_partition(&self) -> Option<String> {
        match blockdev::get_single_device("/") {
            Ok(device) => {
                let bios_boot_part =
                    blockdev::get_bios_boot_partition(&device).expect("get bios_boot part");
                return bios_boot_part;
            }
            Err(e) => log::warn!("Get error: {}", e),
        }
        log::debug!("Not found any bios_boot partition");
        None
    }
}

impl Component for Bios {
    fn name(&self) -> &'static str {
        "BIOS"
    }

    fn install(
        &self,
        src_root: &openat::Dir,
        dest_root: &str,
        device: &str,
        _update_firmware: bool,
    ) -> Result<InstalledContent> {
        let Some(meta) = get_component_update(src_root, self)? else {
            anyhow::bail!("No update metadata for component {} found", self.name());
        };

        self.run_grub_install(dest_root, device)?;
        Ok(InstalledContent {
            meta,
            filetree: None,
            adopted_from: None,
        })
    }

    fn generate_update_metadata(&self, sysroot_path: &str) -> Result<ContentMetadata> {
        let grub_install = Path::new(sysroot_path).join(GRUB_BIN);
        if !grub_install.exists() {
            bail!("Failed to find {:?}", grub_install);
        }

        // Query the rpm database and list the package and build times for /usr/sbin/grub2-install
        let meta = packagesystem::query_files(sysroot_path, [&grub_install])?;
        write_update_metadata(sysroot_path, self, &meta)?;
        Ok(meta)
    }

    fn query_adopt(&self) -> Result<Option<Adoptable>> {
        #[cfg(target_arch = "x86_64")]
        if crate::efi::is_efi_booted()? && self.get_bios_boot_partition().is_none() {
            log::debug!("Skip BIOS adopt");
            return Ok(None);
        }
        crate::component::query_adopt_state()
    }

    fn adopt_update(&self, _: &openat::Dir, update: &ContentMetadata) -> Result<InstalledContent> {
        let Some(meta) = self.query_adopt()? else {
            anyhow::bail!("Failed to find adoptable system")
        };

        let target_root = "/";
        let device = blockdev::get_single_device(&target_root)?;
        self.run_grub_install(target_root, &device)?;
        log::debug!("Install grub modules on {device}");
        Ok(InstalledContent {
            meta: update.clone(),
            filetree: None,
            adopted_from: Some(meta.version),
        })
    }

    fn query_update(&self, sysroot: &openat::Dir) -> Result<Option<ContentMetadata>> {
        get_component_update(sysroot, self)
    }

    fn run_update(&self, sysroot: &openat::Dir, _: &InstalledContent) -> Result<InstalledContent> {
        let updatemeta = self.query_update(sysroot)?.expect("update available");
        let dest_fd = format!("/proc/self/fd/{}", sysroot.as_raw_fd());
        let dest_root = std::fs::read_link(dest_fd)?;
        let device = blockdev::get_single_device(&dest_root)?;

        let dest_root = dest_root.to_string_lossy().into_owned();
        self.run_grub_install(&dest_root, &device)?;
        log::debug!("Install grub modules on {device}");

        let adopted_from = None;
        Ok(InstalledContent {
            meta: updatemeta,
            filetree: None,
            adopted_from,
        })
    }

    fn validate(&self, _: &InstalledContent) -> Result<ValidationResult> {
        Ok(ValidationResult::Skip)
    }

    fn get_efi_vendor(&self, _: &openat::Dir) -> Result<Option<String>> {
        Ok(None)
    }
}
