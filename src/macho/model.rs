use bitflags::bitflags;

// ARM64 is always little-endian 64-bit; the on-disk magic bytes are [CF, FA, ED, FE]
pub const MH_CIGAM_64: u32 = 0xCFFA_EDFE;

pub struct MachoFile {
    pub header: Header,
    pub load_commands: Vec<LoadCommand>,
}

pub struct Header {
    pub cpu_subtype: u32,
    pub file_type: FileType,
    pub flags: Flags,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    Object,
    Execute,
    Dylib,
    Bundle,
    DylibStub,
    Dsym,
    Unknown(u32),
}

impl FileType {
    pub fn from_raw(value: u32) -> Self {
        match value {
            1 => FileType::Object,
            2 => FileType::Execute,
            6 => FileType::Dylib,
            8 => FileType::Bundle,
            9 => FileType::DylibStub,
            10 => FileType::Dsym,
            other => FileType::Unknown(other),
        }
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy)]
    pub struct Flags: u32 {
        const NO_UNDEFS           = 0x0000_0001;
        const INCRLINK            = 0x0000_0002;
        const DYLDLINK            = 0x0000_0004;
        const TWOLEVEL            = 0x0000_0080;
        const PIE                 = 0x0020_0000;
        const HAS_TLV_DESCRIPTORS = 0x0080_0000;
    }
}

pub enum LoadCommand {
    Segment(SegmentCommand),
    SymbolTable(SymtabCommand),
    DyldInfo(DyldInfoCommand),
    Uuid([u8; 16]),
    EntryPoint(EntryPointCommand),
    CodeSignature(LinkEditDataCommand),
    Raw { cmd: u32, data: Vec<u8> },
}

pub struct SegmentCommand {
    pub name: String,
    pub vm_addr: u64,
    pub vm_size: u64,
    pub file_offset: u64,
    pub file_size: u64,
    pub max_prot: u32,
    pub init_prot: u32,
    pub sections: Vec<Section>,
}

pub struct Section {
    pub name: String,
    pub segment_name: String,
    pub addr: u64,
    pub size: u64,
    pub offset: u32,
    pub align: u32,
    pub flags: u32,
}

pub struct SymtabCommand {
    pub sym_offset: u32,
    pub nsyms: u32,
    pub str_offset: u32,
    pub str_size: u32,
}

pub struct DyldInfoCommand {
    pub rebase_offset: u32,
    pub rebase_size: u32,
    pub bind_offset: u32,
    pub bind_size: u32,
    pub export_offset: u32,
    pub export_size: u32,
}

pub struct EntryPointCommand {
    pub entry_offset: u64,
    pub stack_size: u64,
}

pub struct LinkEditDataCommand {
    pub data_offset: u32,
    pub data_size: u32,
}
