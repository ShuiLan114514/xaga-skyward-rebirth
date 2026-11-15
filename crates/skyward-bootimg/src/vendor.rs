//! A vendor boot image with header version 3 or 4 (`vendor_boot`): its
//! header, vendor ramdisk, device tree and, from version 4, its table of
//! ramdisk fragments and its bootconfig. Read, changed and written again by
//! the image packer, for phones whose recovery rides in the vendor boot
//! image instead of a recovery partition of its own.

use abootimg_oxide::binrw::io::Cursor;
use abootimg_oxide::binrw::{BinRead, BinWrite};
use abootimg_oxide::VendorHeader;
use anyhow::{ensure, Context, Result};
use std::fs;
use std::path::Path;

/// One entry of a version 4 vendor boot image's ramdisk table: where a
/// fragment of the vendor ramdisk lies, what kind it is and which boards it
/// is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableEntry {
    pub size: u32,
    pub offset: u32,
    /// 1 for a platform fragment, 3 for a recovery one.
    pub kind: u32,
    pub name: [u8; 32],
    pub board_id: [[u8; 16]; 4],
}

/// The length of a table entry as the format lays it out.
const ENTRY_LEN: usize = 4 + 4 + 4 + 32 + 4 * 16;

fn section(bytes: &[u8], at: usize, len: u32, what: &str) -> Result<Vec<u8>> {
    let end = at.checked_add(len as usize).context("section end")?;
    bytes.get(at..end).map(<[u8]>::to_vec).ok_or_else(|| anyhow::anyhow!("{what} runs past the end of the image"))
}

fn pad(out: &mut Vec<u8>, page: usize) {
    out.resize(out.len().div_ceil(page) * page, 0);
}

impl TableEntry {
    fn parse(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() >= ENTRY_LEN, "a vendor ramdisk table entry is {} bytes", bytes.len());
        let read = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"));
        let mut name = [0u8; 32];
        name.copy_from_slice(&bytes[12..44]);
        let mut board_id = [[0u8; 16]; 4];
        for (n, board) in board_id.iter_mut().enumerate() {
            board.copy_from_slice(&bytes[44 + n * 16..60 + n * 16]);
        }
        Ok(TableEntry { size: read(0), offset: read(4), kind: read(8), name, board_id })
    }

    fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.size.to_le_bytes());
        out.extend_from_slice(&self.offset.to_le_bytes());
        out.extend_from_slice(&self.kind.to_le_bytes());
        out.extend_from_slice(&self.name);
        for board in &self.board_id {
            out.extend_from_slice(board);
        }
    }
}

/// A vendor boot image, split into its header and the sections the header
/// locates.
pub struct VendorBootImage {
    pub header: VendorHeader,
    pub ramdisk: Vec<u8>,
    pub dtb: Vec<u8>,
    pub table: Vec<TableEntry>,
    pub bootconfig: Vec<u8>,
}

impl VendorBootImage {
    pub fn read(path: &Path) -> Result<Self> {
        let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
        VendorBootImage::parse(&bytes).with_context(|| format!("read {}", path.display()))
    }

    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let header = VendorHeader::read(&mut Cursor::new(bytes)).context("header")?;
        ensure!(header.page_size.is_power_of_two() && header.page_size >= 512, "page size {}", header.page_size);
        let ramdisk = section(bytes, header.ramdisk_position(), header.vendor_ramdisk_size, "vendor ramdisk")?;
        let dtb = section(bytes, header.dtb_position(), header.dtb_size, "dtb")?;
        let (table, bootconfig) = match &header.v4 {
            None => (Vec::new(), Vec::new()),
            Some(v4) => {
                let entry = v4.vendor_ramdisk_table_entry_size as usize;
                ensure!(entry >= ENTRY_LEN, "a vendor ramdisk table entry is {entry} bytes");
                let raw = section(bytes, header.ramdisk_table_position(), v4.vendor_ramdisk_table_size, "vendor ramdisk table")?;
                let mut table = Vec::with_capacity(v4.vendor_ramdisk_table_entry_num as usize);
                for n in 0..v4.vendor_ramdisk_table_entry_num as usize {
                    table.push(TableEntry::parse(&raw[n * entry..])?);
                }
                let at = header.bootconfig_position().expect("a version 4 header has a bootconfig position");
                (table, section(bytes, at, v4.bootconfig_size, "bootconfig")?)
            }
        };
        Ok(VendorBootImage { header, ramdisk, dtb, table, bootconfig })
    }

    /// The image with its ramdisk replaced by `ramdisk`, every other section
    /// as it was: one fragment of the same kind as the one the stock image
    /// carried, when it carried one.
    pub fn with_ramdisk(&self, ramdisk: Vec<u8>) -> Result<Self> {
        ensure!(
            self.table.len() <= 1,
            "the vendor boot image carries {} ramdisk fragments, and this packer replaces one",
            self.table.len()
        );
        let mut table = self.table.clone();
        match table.first_mut() {
            Some(entry) => {
                entry.size = u32::try_from(ramdisk.len()).context("vendor ramdisk size")?;
                entry.offset = 0;
            }
            None => table.push(TableEntry { size: ramdisk.len() as u32, offset: 0, kind: 1, name: [0; 32], board_id: [[0; 16]; 4] }),
        }
        Ok(VendorBootImage {
            header: self.header.clone(),
            ramdisk,
            dtb: self.dtb.clone(),
            table,
            bootconfig: self.bootconfig.clone(),
        })
    }

    /// The image as the partition holds it before its AVB footer.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let page = self.header.page_size as usize;
        let mut header = self.header.clone();
        header.vendor_ramdisk_size = u32::try_from(self.ramdisk.len()).context("vendor ramdisk size")?;
        if let Some(v4) = &mut header.v4 {
            v4.vendor_ramdisk_table_entry_num = u32::try_from(self.table.len()).context("table entries")?;
            v4.vendor_ramdisk_table_entry_size = ENTRY_LEN as u32;
            v4.vendor_ramdisk_table_size = u32::try_from(self.table.len() * ENTRY_LEN).context("table size")?;
            v4.bootconfig_size = u32::try_from(self.bootconfig.len()).context("bootconfig size")?;
        }
        let mut writer = Cursor::new(Vec::new());
        header.write(&mut writer).context("write header")?;
        let mut out = writer.into_inner();
        pad(&mut out, page);
        out.extend_from_slice(&self.ramdisk);
        pad(&mut out, page);
        out.extend_from_slice(&self.dtb);
        pad(&mut out, page);
        if self.header.v4.is_some() {
            let mut table = Vec::with_capacity(self.table.len() * ENTRY_LEN);
            for entry in &self.table {
                entry.write(&mut table);
            }
            out.extend_from_slice(&table);
            pad(&mut out, page);
            out.extend_from_slice(&self.bootconfig);
            pad(&mut out, page);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A version 4 vendor boot image with `ramdisk` as its one fragment and
    /// `dtb` as its device tree, laid out as the format lays it out.
    fn image(ramdisk: &[u8], dtb: &[u8]) -> Vec<u8> {
        let header = VendorHeader {
            page_size: 4096,
            kernel_addr: 0,
            ramdisk_addr: 0,
            vendor_ramdisk_size: ramdisk.len() as u32,
            cmdline: Box::new([0; 2048]),
            tags_addr: 0,
            board_name: [0; 16],
            dtb_size: dtb.len() as u32,
            dtb_addr: 0,
            v4: Some(abootimg_oxide::VendorHeaderV4 {
                vendor_ramdisk_table_size: ENTRY_LEN as u32,
                vendor_ramdisk_table_entry_num: 1,
                vendor_ramdisk_table_entry_size: ENTRY_LEN as u32,
                bootconfig_size: 0,
            }),
        };
        let mut writer = Cursor::new(Vec::new());
        header.write(&mut writer).unwrap();
        let mut out = writer.into_inner();
        pad(&mut out, 4096);
        out.extend_from_slice(ramdisk);
        pad(&mut out, 4096);
        out.extend_from_slice(dtb);
        pad(&mut out, 4096);
        let entry = TableEntry { size: ramdisk.len() as u32, offset: 0, kind: 3, name: [0; 32], board_id: [[0; 16]; 4] };
        entry.write(&mut out);
        pad(&mut out, 4096);
        out
    }

    #[test]
    fn a_vendor_boot_image_keeps_its_dtb_and_its_fragment_kind() {
        let dtb = b"dtb";
        let stock = image(b"stock ramdisk", dtb);
        let image = VendorBootImage::parse(&stock).unwrap();
        assert_eq!(image.ramdisk, b"stock ramdisk");
        assert_eq!(image.dtb, dtb);
        assert_eq!(image.table[0].kind, 3, "a recovery fragment stays one");
        let rebuilt = image.with_ramdisk(b"skyward".to_vec()).unwrap();
        let parsed = VendorBootImage::parse(&rebuilt.to_bytes().unwrap()).unwrap();
        assert_eq!(parsed.ramdisk, b"skyward");
        assert_eq!(parsed.dtb, dtb);
        assert_eq!(parsed.table[0].size, 7);
        assert_eq!(parsed.table[0].offset, 0);
        assert_eq!(parsed.header.vendor_ramdisk_size, 7);
    }

    #[test]
    fn an_image_with_more_than_one_fragment_is_refused() {
        let mut image = VendorBootImage::parse(&image(b"ramdisk", b"dtb")).unwrap();
        image.table.push(image.table[0].clone());
        assert!(image.with_ramdisk(b"skyward".to_vec()).is_err());
    }
}
