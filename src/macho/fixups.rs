use anyhow::{bail, Context, Result};

use super::model::{self, LoadCommand, SegmentCommand};

pub const FIXUP_KIND_REBASE: u32 = 0;
pub const FIXUP_KIND_BIND: u32 = 1;

const DYLD_CHAINED_IMPORT: u32 = 1;
const DYLD_CHAINED_PTR_64_OFFSET: u16 = 6;
const DYLD_CHAINED_PTR_START_NONE: u16 = 0xffff;
const DYLD_CHAINED_PTR_START_MULTI: u16 = 0x8000;

#[derive(Debug)]
pub struct PackedImport {
    pub name: String,
    pub weak: bool,
}

#[derive(Debug)]
pub struct PackedFixup {
    pub offset: u64,
    pub target: u64,
    pub addend: u64,
    pub import_index: u32,
    pub high8: u32,
    pub kind: u32,
}

pub fn parse_chained_fixups(
    binary: &[u8],
    macho: &model::MachoFile,
    segments: &[&SegmentCommand],
    min_vm_addr: u64,
) -> Result<(Vec<PackedImport>, Vec<PackedFixup>)> {
    let Some(command) = macho.load_commands.iter().find_map(|lc| {
        if let LoadCommand::ChainedFixups(command) = lc {
            Some(command)
        } else {
            None
        }
    }) else {
        return Ok((Vec::new(), Vec::new()));
    };

    let blob_start = command.data_offset as usize;
    let blob_end = blob_start
        .checked_add(command.data_size as usize)
        .context("LC_DYLD_CHAINED_FIXUPS range overflowed usize")?;
    let blob = binary
        .get(blob_start..blob_end)
        .context("LC_DYLD_CHAINED_FIXUPS extends past end of file")?;

    let fixups_version = read_u32_at(blob, 0)?;
    let starts_offset = read_u32_at(blob, 4)? as usize;
    let imports_offset = read_u32_at(blob, 8)? as usize;
    let symbols_offset = read_u32_at(blob, 12)? as usize;
    let imports_count = read_u32_at(blob, 16)? as usize;
    let imports_format = read_u32_at(blob, 20)?;
    let symbols_format = read_u32_at(blob, 24)?;

    if fixups_version != 0 {
        bail!("Unsupported chained fixups version {fixups_version}");
    }
    if imports_format != DYLD_CHAINED_IMPORT {
        bail!("Unsupported chained imports format {imports_format}");
    }
    if symbols_format != 0 {
        bail!("Unsupported compressed chained import symbol table");
    }

    let imports = parse_chained_imports(blob, imports_offset, symbols_offset, imports_count)?;
    let fixups =
        parse_chained_starts(blob, starts_offset, segments, min_vm_addr, binary, &imports)?;

    Ok((imports, fixups))
}

fn parse_chained_imports(
    blob: &[u8],
    imports_offset: usize,
    symbols_offset: usize,
    imports_count: usize,
) -> Result<Vec<PackedImport>> {
    let mut imports = Vec::with_capacity(imports_count);
    for i in 0..imports_count {
        let raw = read_u32_at(blob, imports_offset + i * size_of::<u32>())?;
        let weak = ((raw >> 8) & 1) != 0;
        let name_offset = (raw >> 9) as usize;
        let name = read_null_terminated(blob, symbols_offset + name_offset)
            .with_context(|| format!("Invalid chained import symbol name at index {i}"))?;
        imports.push(PackedImport { name, weak });
    }
    Ok(imports)
}

fn parse_chained_starts(
    blob: &[u8],
    starts_offset: usize,
    segments: &[&SegmentCommand],
    min_vm_addr: u64,
    binary: &[u8],
    imports: &[PackedImport],
) -> Result<Vec<PackedFixup>> {
    let seg_count = read_u32_at(blob, starts_offset)? as usize;
    if seg_count > segments.len() {
        bail!(
            "Chained fixups reference {seg_count} segments, but Mach-O has only {}",
            segments.len()
        );
    }

    let mut fixups = Vec::new();
    for segment_index in 0..seg_count {
        let seg_info_offset = read_u32_at(blob, starts_offset + 4 + segment_index * 4)? as usize;
        if seg_info_offset == 0 {
            continue;
        }

        let seg = segments[segment_index];
        let starts = starts_offset + seg_info_offset;
        let _size = read_u32_at(blob, starts)?;
        let page_size = read_u16_at(blob, starts + 4)? as u64;
        let pointer_format = read_u16_at(blob, starts + 6)?;
        let segment_offset = read_u64_at(blob, starts + 8)?;
        let _max_valid_pointer = read_u32_at(blob, starts + 16)?;
        let page_count = read_u16_at(blob, starts + 20)? as usize;

        if pointer_format != DYLD_CHAINED_PTR_64_OFFSET {
            bail!(
                "Unsupported chained pointer format {pointer_format} in segment {}",
                seg.name
            );
        }
        let expected_segment_offset = seg.vm_addr - min_vm_addr;
        if segment_offset != expected_segment_offset {
            bail!(
                "Unsupported chained fixup segment offset for {}: got {segment_offset:#x}, expected {expected_segment_offset:#x}",
                seg.name
            );
        }

        for page_index in 0..page_count {
            let page_start = read_u16_at(blob, starts + 22 + page_index * 2)?;
            if page_start == DYLD_CHAINED_PTR_START_NONE {
                continue;
            }
            if (page_start & DYLD_CHAINED_PTR_START_MULTI) != 0 {
                bail!("Unsupported chained fixups with multiple starts per page");
            }

            let mut fixup_offset =
                segment_offset + page_index as u64 * page_size + page_start as u64;
            loop {
                let raw = read_u64_at(
                    binary,
                    file_offset_for_image_offset(segments, min_vm_addr, fixup_offset)?,
                )?;
                let bind = (raw >> 63) != 0;
                let next = (raw >> 51) & 0x0fff;

                if bind {
                    let import_index = (raw & 0x00ff_ffff) as u32;
                    if import_index as usize >= imports.len() {
                        bail!("Chained fixup references invalid import index {import_index}");
                    }
                    let addend = (raw >> 24) & 0xff;
                    fixups.push(PackedFixup {
                        offset: fixup_offset,
                        target: 0,
                        addend,
                        import_index,
                        high8: 0,
                        kind: FIXUP_KIND_BIND,
                    });
                } else {
                    fixups.push(PackedFixup {
                        offset: fixup_offset,
                        target: raw & 0x0000_000f_ffff_ffff,
                        addend: 0,
                        import_index: 0,
                        high8: ((raw >> 36) & 0xff) as u32,
                        kind: FIXUP_KIND_REBASE,
                    });
                }

                if next == 0 {
                    break;
                }
                fixup_offset = fixup_offset
                    .checked_add(next * 4)
                    .context("Chained fixup offset overflowed")?;
            }
        }
    }

    Ok(fixups)
}

fn file_offset_for_image_offset(
    segments: &[&SegmentCommand],
    min_vm_addr: u64,
    image_offset: u64,
) -> Result<usize> {
    for seg in segments {
        if seg.name == "__PAGEZERO" || seg.name == "__LINKEDIT" || seg.file_size == 0 {
            continue;
        }
        let start = seg.vm_addr - min_vm_addr;
        let end = start + seg.file_size;
        if image_offset >= start && image_offset + size_of::<u64>() as u64 <= end {
            return Ok((seg.file_offset + (image_offset - start)) as usize);
        }
    }
    bail!("Chained fixup at image offset {image_offset:#x} is outside file-backed segments")
}

fn read_u16_at(data: &[u8], offset: usize) -> Result<u16> {
    let bytes = data
        .get(offset..offset + 2)
        .with_context(|| format!("unexpected end of data at offset {offset}"))?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

pub fn read_u32_at(data: &[u8], offset: usize) -> Result<u32> {
    let bytes = data
        .get(offset..offset + 4)
        .with_context(|| format!("unexpected end of data at offset {offset}"))?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_u64_at(data: &[u8], offset: usize) -> Result<u64> {
    let bytes = data
        .get(offset..offset + 8)
        .with_context(|| format!("unexpected end of data at offset {offset}"))?;
    Ok(u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}

fn read_null_terminated(data: &[u8], offset: usize) -> Result<String> {
    let bytes = data
        .get(offset..)
        .with_context(|| format!("unexpected end of data at offset {offset}"))?;
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .context("unterminated string")?;
    Ok(String::from_utf8_lossy(&bytes[..end]).into_owned())
}
