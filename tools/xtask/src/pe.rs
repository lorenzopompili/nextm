//! Parser minimale di file PE32+ (x64 e ARM64) per i controlli di CI: tabella
//! degli import, delay-import e Load Config Directory.
//!
//! Lavora soltanto sui byte del file: nessuna chiamata di sistema, quindi gira
//! ovunque e si prova con PE sintetici.
#![forbid(unsafe_code)]

use std::fmt;

const DOS_MAGIC: u16 = 0x5A4D; // "MZ"
const NT_SIGNATURE: u32 = 0x0000_4550; // "PE\0\0"
const OPTIONAL_MAGIC_PE32_PLUS: u16 = 0x20B;

/// Macchine riconosciute (IMAGE_FILE_MACHINE_*), solo per la stampa.
const MACHINE_AMD64: u16 = 0x8664;
const MACHINE_ARM64: u16 = 0xAA64;

/// Indici nella tabella delle Data Directory (IMAGE_DIRECTORY_ENTRY_*).
const DIR_IMPORT: usize = 1;
const DIR_LOAD_CONFIG: usize = 10;
const DIR_DELAY_IMPORT: usize = 13;

/// Parte fissa di IMAGE_OPTIONAL_HEADER64, prima delle Data Directory.
const OPTIONAL_HEADER64_FIXED: usize = 112;
/// Dimensione di IMAGE_SECTION_HEADER.
const SECTION_HEADER_SIZE: usize = 40;
/// Dimensione di IMAGE_IMPORT_DESCRIPTOR.
const IMPORT_DESCRIPTOR_SIZE: usize = 20;
/// Dimensione di ImgDelayDescr.
const DELAY_DESCRIPTOR_SIZE: usize = 32;

/// Offset di `DependentLoadFlags` (u16) in IMAGE_LOAD_CONFIG_DIRECTORY64.
const DEPENDENT_LOAD_FLAGS_OFFSET: usize = 78;

/// Limiti contro file corrotti: niente cicli infiniti né stringhe senza fine.
const MAX_DESCRIPTORS: usize = 4096;
const MAX_NAME_LEN: usize = 512;

/// Errori di lettura del PE.
#[derive(Debug, PartialEq, Eq)]
pub enum PeError {
    /// I byte finiscono prima del previsto (cosa si stava leggendo).
    Truncated(&'static str),
    /// Il file non è un PE valido (motivo).
    Invalid(&'static str),
    /// Non è un PE32+ (valore del campo Magic dell'Optional Header).
    Unsupported(u16),
    /// La RVA non cade in nessuna sezione con dati nel file.
    RvaOutOfSection(u32),
    /// La Load Config Directory non c'è.
    NoLoadConfig,
    /// La Load Config Directory è troppo corta per contenere `DependentLoadFlags`
    /// (dimensione trovata, in byte).
    LoadConfigTooShort(u32),
}

impl fmt::Display for PeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PeError::Truncated(what) => write!(f, "file troncato mentre si legge: {what}"),
            PeError::Invalid(why) => write!(f, "PE non valido: {why}"),
            PeError::Unsupported(magic) => {
                write!(f, "formato non supportato (Magic 0x{magic:04X}): serve un PE32+ (x64 o ARM64)")
            }
            PeError::RvaOutOfSection(rva) => {
                write!(f, "la RVA 0x{rva:08X} non cade in nessuna sezione del file")
            }
            PeError::NoLoadConfig => write!(f, "Load Config Directory assente"),
            PeError::LoadConfigTooShort(size) => {
                write!(f, "Load Config Directory troppo corta ({size} byte): manca DependentLoadFlags")
            }
        }
    }
}

/// Legge `N` byte a `off`, con controllo dei limiti.
fn bytes<const N: usize>(data: &[u8], off: usize, what: &'static str) -> Result<[u8; N], PeError> {
    off.checked_add(N)
        .and_then(|end| data.get(off..end))
        .and_then(|slice| <[u8; N]>::try_from(slice).ok())
        .ok_or(PeError::Truncated(what))
}

fn rd_u16(data: &[u8], off: usize, what: &'static str) -> Result<u16, PeError> {
    bytes::<2>(data, off, what).map(u16::from_le_bytes)
}

fn rd_u32(data: &[u8], off: usize, what: &'static str) -> Result<u32, PeError> {
    bytes::<4>(data, off, what).map(u32::from_le_bytes)
}

/// Voce della tabella delle sezioni (solo i campi che servono).
#[derive(Debug, Clone, Copy)]
struct Section {
    virtual_address: u32,
    virtual_size: u32,
    raw_size: u32,
    raw_ptr: u32,
}

/// File PE32+ già interpretato: intestazioni, sezioni e Data Directory.
pub struct Pe<'a> {
    data: &'a [u8],
    machine: u16,
    size_of_headers: u32,
    sections: Vec<Section>,
    /// (RVA, dimensione) di ogni Data Directory presente.
    dirs: Vec<(u32, u32)>,
}

impl<'a> Pe<'a> {
    /// Interpreta DOS header, NT headers, Optional Header, Data Directory e
    /// tabella delle sezioni. Accetta solo PE32+.
    pub fn parse(data: &'a [u8]) -> Result<Pe<'a>, PeError> {
        if rd_u16(data, 0, "intestazione DOS")? != DOS_MAGIC {
            return Err(PeError::Invalid("manca la firma MZ"));
        }
        let nt = rd_u32(data, 0x3C, "e_lfanew")? as usize;
        if rd_u32(data, nt, "firma PE")? != NT_SIGNATURE {
            return Err(PeError::Invalid("manca la firma PE"));
        }
        // IMAGE_FILE_HEADER (20 byte) subito dopo la firma.
        let file_header = nt + 4;
        let machine = rd_u16(data, file_header, "Machine")?;
        let section_count = rd_u16(data, file_header + 2, "NumberOfSections")? as usize;
        let optional_size = rd_u16(data, file_header + 16, "SizeOfOptionalHeader")? as usize;
        let optional = file_header + 20;

        let magic = rd_u16(data, optional, "Magic")?;
        if magic != OPTIONAL_MAGIC_PE32_PLUS {
            return Err(PeError::Unsupported(magic));
        }
        let size_of_headers = rd_u32(data, optional + 60, "SizeOfHeaders")?;
        let declared_dirs = rd_u32(data, optional + 108, "NumberOfRvaAndSizes")? as usize;

        // Le Data Directory (al massimo 16) devono stare dentro l'Optional Header.
        let room = optional_size.saturating_sub(OPTIONAL_HEADER64_FIXED) / 8;
        let dir_count = declared_dirs.min(16).min(room);
        let mut dirs = Vec::with_capacity(dir_count);
        for i in 0..dir_count {
            let at = optional + OPTIONAL_HEADER64_FIXED + i * 8;
            dirs.push((rd_u32(data, at, "Data Directory")?, rd_u32(data, at + 4, "Data Directory")?));
        }

        // La tabella delle sezioni segue l'Optional Header.
        let table = optional + optional_size;
        let mut sections = Vec::with_capacity(section_count.min(96));
        for i in 0..section_count {
            let at = table + i * SECTION_HEADER_SIZE;
            sections.push(Section {
                virtual_size: rd_u32(data, at + 8, "tabella delle sezioni")?,
                virtual_address: rd_u32(data, at + 12, "tabella delle sezioni")?,
                raw_size: rd_u32(data, at + 16, "tabella delle sezioni")?,
                raw_ptr: rd_u32(data, at + 20, "tabella delle sezioni")?,
            });
        }

        // Le intestazioni occupano al più lo spazio che precede la prima sezione:
        // un SizeOfHeaders esagerato non deve far "mappare" RVA qualsiasi.
        let headers_end = sections
            .iter()
            .map(|s| s.virtual_address)
            .min()
            .map_or(size_of_headers, |first| first.min(size_of_headers));

        Ok(Pe { data, machine, size_of_headers: headers_end, sections, dirs })
    }

    /// Nome leggibile dell'architettura (`x64`, `ARM64` o il codice esadecimale).
    pub fn machine_name(&self) -> String {
        match self.machine {
            MACHINE_AMD64 => "x64".to_string(),
            MACHINE_ARM64 => "ARM64".to_string(),
            other => format!("0x{other:04X}"),
        }
    }

    /// Converte una RVA in offset nel file.
    ///
    /// Vale solo per gli indirizzi che hanno byte nel file: le intestazioni
    /// (mappate all'inizio dell'immagine) e la parte di ogni sezione coperta da
    /// `SizeOfRawData` e da `VirtualSize`. Una RVA fuori da queste zone (buchi tra
    /// le sezioni, coda azzerata di una sezione, oltre l'immagine) è un errore.
    pub fn rva_to_offset(&self, rva: u32) -> Result<usize, PeError> {
        if rva < self.size_of_headers {
            return Ok(rva as usize);
        }
        for s in &self.sections {
            // Con VirtualSize a 0 (vecchi strumenti) fa fede SizeOfRawData.
            let extent = if s.virtual_size == 0 { s.raw_size } else { s.virtual_size };
            let backed = extent.min(s.raw_size);
            if let Some(delta) = rva.checked_sub(s.virtual_address)
                && delta < backed
            {
                return Ok(s.raw_ptr as usize + delta as usize);
            }
        }
        Err(PeError::RvaOutOfSection(rva))
    }

    /// (RVA, dimensione) della Data Directory `index`, se presente e non vuota.
    fn dir(&self, index: usize) -> Option<(u32, u32)> {
        self.dirs.get(index).copied().filter(|&(rva, _)| rva != 0)
    }

    /// Stringa ASCII terminata da NUL a `offset`.
    fn c_string(&self, offset: usize) -> Result<String, PeError> {
        let tail = self.data.get(offset..).ok_or(PeError::Truncated("nome di DLL"))?;
        let len = tail
            .iter()
            .take(MAX_NAME_LEN)
            .position(|&b| b == 0)
            .ok_or(PeError::Invalid("nome di DLL senza terminatore"))?;
        Ok(String::from_utf8_lossy(&tail[..len]).into_owned())
    }

    /// Nomi delle DLL nella tabella degli import, nell'ordine del file.
    ///
    /// Come il loader, la lettura si ferma al primo descrittore senza nome.
    pub fn imports(&self) -> Result<Vec<String>, PeError> {
        let Some((rva, _)) = self.dir(DIR_IMPORT) else {
            return Ok(Vec::new());
        };
        let mut at = self.rva_to_offset(rva)?;
        let mut names = Vec::new();
        for _ in 0..MAX_DESCRIPTORS {
            let name_rva = rd_u32(self.data, at + 12, "descrittore di import")?;
            if name_rva == 0 {
                return Ok(names);
            }
            names.push(self.c_string(self.rva_to_offset(name_rva)?)?);
            at += IMPORT_DESCRIPTOR_SIZE;
        }
        Err(PeError::Invalid("troppi descrittori di import"))
    }

    /// Nomi delle DLL nella Delay Import Directory (vuoto se non c'è nessun
    /// descrittore prima del terminatore a zero).
    pub fn delay_imports(&self) -> Result<Vec<String>, PeError> {
        let Some((rva, _)) = self.dir(DIR_DELAY_IMPORT) else {
            return Ok(Vec::new());
        };
        let mut at = self.rva_to_offset(rva)?;
        let mut names = Vec::new();
        for _ in 0..MAX_DESCRIPTORS {
            let desc = bytes::<DELAY_DESCRIPTOR_SIZE>(self.data, at, "descrittore di delay-import")?;
            if desc.iter().all(|&b| b == 0) {
                return Ok(names);
            }
            // rvaDLLName è il secondo campo (offset 4). Se il nome non si legge il
            // descrittore conta comunque: la directory non è vuota.
            let name_rva = u32::from_le_bytes([desc[4], desc[5], desc[6], desc[7]]);
            let name = self
                .rva_to_offset(name_rva)
                .and_then(|off| self.c_string(off))
                .unwrap_or_else(|_| "<nome non leggibile>".to_string());
            names.push(name);
            at += DELAY_DESCRIPTOR_SIZE;
        }
        Err(PeError::Invalid("troppi descrittori di delay-import"))
    }

    /// Campo `DependentLoadFlags` della Load Config Directory.
    ///
    /// Errore se la directory manca o se è troppo corta (sia la dimensione nella
    /// Data Directory sia il campo `Size` della struttura devono coprire il campo).
    pub fn dependent_load_flags(&self) -> Result<u16, PeError> {
        let (rva, dir_size) = self.dir(DIR_LOAD_CONFIG).ok_or(PeError::NoLoadConfig)?;
        let at = self.rva_to_offset(rva)?;
        let struct_size = rd_u32(self.data, at, "Load Config Directory")?;
        let needed = (DEPENDENT_LOAD_FLAGS_OFFSET + 2) as u32;
        if struct_size < needed || dir_size < needed {
            return Err(PeError::LoadConfigTooShort(struct_size.min(dir_size)));
        }
        rd_u16(self.data, at + DEPENDENT_LOAD_FLAGS_OFFSET, "DependentLoadFlags")
    }
}

/// Costruzione di PE sintetici per i test (qui e in `check_imports`).
#[cfg(test)]
pub(crate) mod testutil {
    /// Contenuto della Load Config Directory del PE sintetico.
    #[derive(Clone, Copy)]
    pub enum LoadConfig {
        /// Nessuna Load Config Directory.
        Missing,
        /// Directory di soli 64 byte: `DependentLoadFlags` non c'è.
        Short,
        /// Directory completa (0x140 byte) con questo valore di `DependentLoadFlags`.
        Flags(u16),
    }

    pub struct Spec<'a> {
        pub imports: &'a [&'a str],
        /// `None`: nessuna Delay Import Directory. `Some(&[])`: directory che
        /// contiene solo il terminatore (vuota).
        pub delay: Option<&'a [&'a str]>,
        pub load_config: LoadConfig,
    }

    const SECTION_RVA: u32 = 0x1000;
    const HEADERS_SIZE: usize = 0x200;

    fn put_u16(buf: &mut [u8], off: usize, v: u16) {
        buf[off..off + 2].copy_from_slice(&v.to_le_bytes());
    }

    fn put_u32(buf: &mut [u8], off: usize, v: u32) {
        buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }

    /// Aggiunge una stringa alla sezione e ne restituisce la RVA.
    fn add_name(sec: &mut Vec<u8>, name: &str) -> u32 {
        let rva = SECTION_RVA + sec.len() as u32;
        sec.extend_from_slice(name.as_bytes());
        sec.push(0);
        rva
    }

    /// PE32+ x64 minimo: intestazioni da 0x200 byte e una sola sezione `.rdata`
    /// (RVA 0x1000, offset nel file 0x200) con import, delay-import e Load Config.
    pub fn build_pe(spec: &Spec) -> Vec<u8> {
        // Contenuto della sezione: descrittori, Load Config, poi i nomi.
        let mut sec: Vec<u8> = Vec::new();
        let import_at = sec.len();
        sec.resize(import_at + (spec.imports.len() + 1) * 20, 0);
        let delay_at = sec.len();
        if let Some(delay) = spec.delay {
            sec.resize(delay_at + (delay.len() + 1) * 32, 0);
        }
        while !sec.len().is_multiple_of(8) {
            sec.push(0);
        }
        let config_at = sec.len();
        let config_len = match spec.load_config {
            LoadConfig::Missing => 0,
            LoadConfig::Short => 64,
            LoadConfig::Flags(_) => 0x140,
        };
        sec.resize(config_at + config_len, 0);

        for (i, name) in spec.imports.iter().enumerate() {
            let rva = add_name(&mut sec, name);
            let d = import_at + i * 20;
            put_u32(&mut sec, d + 12, rva); // Name
            put_u32(&mut sec, d + 16, 0x2000); // FirstThunk (non zero)
        }
        for (i, name) in spec.delay.unwrap_or(&[]).iter().enumerate() {
            let rva = add_name(&mut sec, name);
            let d = delay_at + i * 32;
            put_u32(&mut sec, d, 1); // grAttrs: dlattrRva
            put_u32(&mut sec, d + 4, rva); // rvaDLLName
        }
        match spec.load_config {
            LoadConfig::Missing => {}
            LoadConfig::Short => put_u32(&mut sec, config_at, 64),
            LoadConfig::Flags(flags) => {
                put_u32(&mut sec, config_at, 0x140);
                put_u16(&mut sec, config_at + 78, flags);
            }
        }

        // Intestazioni.
        let raw_size = sec.len().next_multiple_of(0x200);
        let mut file = vec![0u8; HEADERS_SIZE];
        file[0..2].copy_from_slice(b"MZ");
        put_u32(&mut file, 0x3C, 0x40); // e_lfanew
        file[0x40..0x44].copy_from_slice(b"PE\0\0");
        let file_header = 0x44;
        put_u16(&mut file, file_header, 0x8664); // Machine: x64
        put_u16(&mut file, file_header + 2, 1); // NumberOfSections
        put_u16(&mut file, file_header + 16, 0xF0); // SizeOfOptionalHeader
        put_u16(&mut file, file_header + 18, 0x22); // Characteristics
        let optional = file_header + 20;
        put_u16(&mut file, optional, 0x20B); // Magic: PE32+
        put_u32(&mut file, optional + 32, 0x1000); // SectionAlignment
        put_u32(&mut file, optional + 36, 0x200); // FileAlignment
        put_u32(&mut file, optional + 56, SECTION_RVA + raw_size.next_multiple_of(0x1000) as u32);
        put_u32(&mut file, optional + 60, HEADERS_SIZE as u32); // SizeOfHeaders
        put_u32(&mut file, optional + 108, 16); // NumberOfRvaAndSizes

        let dirs = optional + 112;
        let mut set_dir = |index: usize, at: usize, size: usize| {
            put_u32(&mut file, dirs + index * 8, SECTION_RVA + at as u32);
            put_u32(&mut file, dirs + index * 8 + 4, size as u32);
        };
        set_dir(1, import_at, (spec.imports.len() + 1) * 20);
        if let Some(delay) = spec.delay {
            set_dir(13, delay_at, (delay.len() + 1) * 32);
        }
        if !matches!(spec.load_config, LoadConfig::Missing) {
            set_dir(10, config_at, config_len);
        }

        // Tabella delle sezioni, subito dopo l'Optional Header.
        let header = optional + 0xF0;
        file[header..header + 6].copy_from_slice(b".rdata");
        put_u32(&mut file, header + 8, sec.len() as u32); // VirtualSize
        put_u32(&mut file, header + 12, SECTION_RVA); // VirtualAddress
        put_u32(&mut file, header + 16, raw_size as u32); // SizeOfRawData
        put_u32(&mut file, header + 20, HEADERS_SIZE as u32); // PointerToRawData
        put_u32(&mut file, header + 36, 0x4000_0040); // Characteristics

        file.extend_from_slice(&sec);
        file.resize(HEADERS_SIZE + raw_size, 0);
        file
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::{LoadConfig, Spec, build_pe};
    use super::*;

    fn own_exe_bytes() -> Vec<u8> {
        let exe = std::env::current_exe().unwrap();
        std::fs::read(exe).unwrap()
    }

    #[test]
    fn own_exe_imports_kernel32() {
        let data = own_exe_bytes();
        let pe = Pe::parse(&data).unwrap();
        let imports = pe.imports().unwrap();
        assert!(imports.iter().any(|n| n.eq_ignore_ascii_case("kernel32.dll")), "import trovati: {imports:?}");
    }

    #[test]
    fn own_exe_has_no_delay_imports_and_reports_machine() {
        let data = own_exe_bytes();
        let pe = Pe::parse(&data).unwrap();
        assert!(pe.delay_imports().unwrap().is_empty());
        let machine = pe.machine_name();
        assert!(machine == "x64" || machine == "ARM64", "macchina: {machine}");
    }

    #[test]
    fn rva_outside_sections_is_error() {
        let data = own_exe_bytes();
        let pe = Pe::parse(&data).unwrap();
        // Lontanissimo dall'immagine.
        assert_eq!(pe.rva_to_offset(0xFFFF_FFF0), Err(PeError::RvaOutOfSection(0xFFFF_FFF0)));
        // Subito dopo l'ultima sezione.
        let end = pe.sections.iter().map(|s| s.virtual_address + s.virtual_size.max(s.raw_size)).max().unwrap();
        assert!(matches!(pe.rva_to_offset(end), Err(PeError::RvaOutOfSection(_))));
        // L'inizio della prima sezione invece si converte nel suo offset nel file.
        let first = pe.sections[0];
        assert_eq!(pe.rva_to_offset(first.virtual_address), Ok(first.raw_ptr as usize));
    }

    #[test]
    fn rva_mapping_rules() {
        let pe = Pe {
            data: &[],
            machine: MACHINE_AMD64,
            size_of_headers: 0x200,
            sections: vec![
                // Sezione con byte in più nel file (padding oltre VirtualSize).
                Section { virtual_address: 0x1000, virtual_size: 0x300, raw_size: 0x400, raw_ptr: 0x200 },
                // Sezione con coda azzerata (VirtualSize oltre SizeOfRawData).
                Section { virtual_address: 0x2000, virtual_size: 0x800, raw_size: 0x200, raw_ptr: 0x600 },
            ],
            dirs: Vec::new(),
        };
        assert_eq!(pe.rva_to_offset(0x10), Ok(0x10)); // intestazioni
        assert_eq!(pe.rva_to_offset(0x1000), Ok(0x200));
        assert_eq!(pe.rva_to_offset(0x12FF), Ok(0x4FF));
        assert!(pe.rva_to_offset(0x1300).is_err()); // oltre VirtualSize
        assert!(pe.rva_to_offset(0x1800).is_err()); // buco tra le sezioni
        assert_eq!(pe.rva_to_offset(0x2000), Ok(0x600));
        assert_eq!(pe.rva_to_offset(0x21FF), Ok(0x7FF));
        assert!(pe.rva_to_offset(0x2200).is_err()); // coda azzerata: nessun byte nel file
    }

    #[test]
    fn rejects_garbage() {
        assert!(matches!(Pe::parse(&[]), Err(PeError::Truncated(_))));
        assert!(matches!(Pe::parse(b"MZ"), Err(PeError::Truncated(_))));
        assert!(matches!(Pe::parse(&[0u8; 256]), Err(PeError::Invalid(_))));
        // Firma MZ giusta ma e_lfanew punta fuori dal file.
        let mut fake = vec![0u8; 64];
        fake[0..2].copy_from_slice(b"MZ");
        fake[0x3C..0x40].copy_from_slice(&0x1000u32.to_le_bytes());
        assert!(matches!(Pe::parse(&fake), Err(PeError::Truncated(_))));
    }

    #[test]
    fn oversized_size_of_headers_does_not_map_every_rva() {
        let mut data = build_pe(&Spec { imports: &[], delay: None, load_config: LoadConfig::Missing });
        let size_of_headers_at = 0x44 + 20 + 60;
        data[size_of_headers_at..size_of_headers_at + 4].copy_from_slice(&0xFFFF_0000u32.to_le_bytes());
        let pe = Pe::parse(&data).unwrap();
        // 0x80 è nelle intestazioni; 0x5000 sta oltre l'unica sezione (0x1000..0x2000).
        assert_eq!(pe.rva_to_offset(0x80), Ok(0x80));
        assert!(pe.rva_to_offset(0x5000).is_err());
    }

    #[test]
    fn rejects_pe32() {
        let mut data = build_pe(&Spec { imports: &[], delay: None, load_config: LoadConfig::Missing });
        let magic_at = 0x44 + 20;
        data[magic_at..magic_at + 2].copy_from_slice(&0x10Bu16.to_le_bytes());
        assert!(matches!(Pe::parse(&data), Err(PeError::Unsupported(0x10B))));
    }

    #[test]
    fn synthetic_imports_and_names_are_read() {
        let data =
            build_pe(&Spec { imports: &["KERNEL32.dll", "user32.dll"], delay: None, load_config: LoadConfig::Missing });
        let pe = Pe::parse(&data).unwrap();
        assert_eq!(pe.machine_name(), "x64");
        assert_eq!(pe.imports().unwrap(), ["KERNEL32.dll", "user32.dll"]);
        assert!(pe.delay_imports().unwrap().is_empty());
    }

    #[test]
    fn synthetic_delay_imports() {
        // Directory presente con il solo terminatore: vuota.
        let empty = build_pe(&Spec { imports: &["kernel32.dll"], delay: Some(&[]), load_config: LoadConfig::Missing });
        assert!(Pe::parse(&empty).unwrap().delay_imports().unwrap().is_empty());
        // Con due descrittori: i nomi vengono elencati.
        let full = build_pe(&Spec {
            imports: &["kernel32.dll"],
            delay: Some(&["dxcore.dll", "iphlpapi.dll"]),
            load_config: LoadConfig::Missing,
        });
        assert_eq!(Pe::parse(&full).unwrap().delay_imports().unwrap(), ["dxcore.dll", "iphlpapi.dll"]);
    }

    #[test]
    fn synthetic_dependent_load_flags() {
        let ok = build_pe(&Spec { imports: &[], delay: None, load_config: LoadConfig::Flags(0x0800) });
        assert_eq!(Pe::parse(&ok).unwrap().dependent_load_flags(), Ok(0x0800));

        let zero = build_pe(&Spec { imports: &[], delay: None, load_config: LoadConfig::Flags(0) });
        assert_eq!(Pe::parse(&zero).unwrap().dependent_load_flags(), Ok(0));

        let missing = build_pe(&Spec { imports: &[], delay: None, load_config: LoadConfig::Missing });
        assert_eq!(Pe::parse(&missing).unwrap().dependent_load_flags(), Err(PeError::NoLoadConfig));

        let short = build_pe(&Spec { imports: &[], delay: None, load_config: LoadConfig::Short });
        assert_eq!(Pe::parse(&short).unwrap().dependent_load_flags(), Err(PeError::LoadConfigTooShort(64)));
    }

    #[test]
    fn truncated_name_string_is_error() {
        let mut data = build_pe(&Spec { imports: &["kernel32.dll"], delay: None, load_config: LoadConfig::Missing });
        // Toglie tutto ciò che segue l'inizio del nome: la stringa non ha più terminatore.
        let name_at = data.windows(12).position(|w| w == b"kernel32.dll").unwrap();
        data.truncate(name_at + 6);
        assert!(Pe::parse(&data).unwrap().imports().is_err());
    }
}
