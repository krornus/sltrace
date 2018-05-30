extern crate rdb;
extern crate addr2line;
extern crate fallible_iterator;
extern crate gimli;
extern crate memmap;
extern crate object;
extern crate x86asm;

use std::fs::File;
use std::io::{self,Seek,SeekFrom};

use fallible_iterator::FallibleIterator;
use x86asm::{InstructionReader,Mode};
use rdb::debugger::Debugger;
use object::Object;

fn main() -> io::Result<()> {
    let dbg = Debugger::new(
        "/home/spowell/programming/c/feh/src/feh",
        vec![
            "--bg-center",
            "/home/spowell/pictures/backgrounds/darkstar_poster.jpg",
        ]
    );

    let addr = 0x155c3;

    let mut file = File::open("/home/spowell/programming/c/feh/src/feh").unwrap();

    let reader_file = file.try_clone()
        .expect("failed to clone file handler");

    let map = unsafe { memmap::Mmap::map(&file).unwrap() };
    let mut reader = InstructionReader::new(reader_file, Mode::Real);

    file.seek(SeekFrom::Start(addr))?;
    let ins = reader.read();
    println!("{:?}",ins);


    trace();

    let obj = &object::File::parse(&*map).unwrap();

    Ok(())
}

struct CompilationUnits<'input, Endian, T = usize>
where
    Endian: gimli::Endianity + 'input,
    T: 'input,
{
    units: Vec<UnitData<'input, Endian, T>>,
    compile_unit: Option<&'input UnitData<'input, Endian, T>>,
}

struct UnitData<'input, Endian, T = usize>
where
    Endian: gimli::Endianity
{
    abbreviations: gimli::Abbreviations,
    name: Option<gimli::EndianBuf<'input, Endian>>,
    comp_dir: Option<gimli::EndianBuf<'input, Endian>>,
    tag: gimli::DwTag,
    low_pc: Option<u64>,
    address_size: u8,
    line_offset: Option<gimli::DebugLineOffset<T>>,
}

fn trace() -> Result<(), Error> {

    let mut file = File::open("/home/spowell/programming/c/feh/src/feh").unwrap();
    let map = unsafe { memmap::Mmap::map(&file).unwrap() };
    let obj = object::File::parse(&*map).unwrap();

    let debug_info:   gimli::DebugInfo<_>   = load_section(&obj)?;
    let debug_abbrev: gimli::DebugAbbrev<_> = load_section(&obj)?;
    let debug_str:    gimli::DebugStr<_>    = load_section(&obj)?;
    let debug_line:    gimli::DebugLine<_>  = load_section(&obj)?;


    let mut compile_idx = None;
    let mut idx = 0;
    let mut iter = debug_info.units();
    let capacity = match iter.size_hint() {
        (_, Some(bound)) => bound,
        (bound, None) => bound,
    };
    let mut units = Vec::with_capacity(capacity);
    loop {
        match iter.next() {
            Ok(Some(unit)) => {

                let (mut name, mut comp_dir): (Option<gimli::EndianBuf<_>>, Option<gimli::EndianBuf<_>>);
                let (mut low, mut size): (Option<u64>, u8);
                let mut tag: gimli::DwTag;
                let mut dbg_line: Option<gimli::DebugLineOffset<_>> = None;

                let abbrevs: gimli::Abbreviations = unit.abbreviations(&debug_abbrev)?;
                {
                    let mut cursor = unit.entries(&abbrevs);
                    let (_,entry) = cursor.next_dfs()?.unwrap();


                    /* load name */
                    let attr = entry.attr_value(gimli::DW_AT_name)?.unwrap();
                    name = if let gimli::AttributeValue::DebugStrRef(offset) = attr {
                        debug_str.get_str(offset).ok()
                    } else {
                        None
                    };

                    /* load comp dir */
                    let attr = entry.attr_value(gimli::DW_AT_comp_dir)?.unwrap();
                    comp_dir = if let gimli::AttributeValue::DebugStrRef(offset) = attr {
                        debug_str.get_str(offset).ok()
                    } else {
                        None
                    };

                    /* load program */
                    let attr = entry.attr_value(gimli::DW_AT_stmt_list)?.unwrap();
                    if let gimli::AttributeValue::DebugLineRef(offset) = attr {
                        dbg_line = Some(offset);
                    }

                    low = if let Some(gimli::AttributeValue::Addr(addr)) =
                        entry.attr_value(gimli::DW_AT_low_pc)?
                    {
                        Some(addr)
                    } else {
                        None
                    };

                    tag = entry.tag();

                    if tag == gimli::DW_TAG_compile_unit {
                        compile_idx = Some(idx);
                    }
                    idx += 1;
                    size = unit.address_size();
                }

                let udata = UnitData {
                    abbreviations: abbrevs,
                    name, comp_dir, tag,
                    low_pc: low,
                    address_size: size,
                    line_offset: dbg_line,
                };

                units.push(udata);
            },
            Ok(None) => { break; },
            _ => { continue; },
        }

    }

    let mut units = CompilationUnits {
        units: units,
        compile_unit: None,
    };

    if let Some(idx) = compile_idx {
        units.compile_unit = Some(&units.units[idx]);
    }

    let base_addr = units.compile_unit.unwrap().low_pc;

    for udata in units.units.iter() {

    }

    Ok(())
}

trait Endianness {
    fn endianness(&self) -> gimli::RunTimeEndian;
}

impl<'data, 'file, Object> Endianness for Object
where
    'data: 'file,
    Object: object::Object<'data, 'file>
{
    fn endianness(&self) -> gimli::RunTimeEndian {
        match self.is_little_endian() {
            true => gimli::RunTimeEndian::Little,
            false => gimli::RunTimeEndian::Big,
        }
    }
}

fn load_section<'input, 'file, Section>(file: &object::File<'input>) -> Result<Section, Error>
where
    'input: 'file,
    Section: gimli::Section<gimli::EndianBuf<'input, gimli::RunTimeEndian>>,
{
    let endian = file.endianness();
    let name = Section::section_name();
    let buf = file.section_data_by_name(name)
        .ok_or(Error::MissingSection)?;
    let reader = gimli::EndianBuf::new(&buf, endian);

    Ok(Section::from(reader))
}


enum Error {
    MissingSection,
    Gimli(gimli::Error),
}


impl From<gimli::Error> for Error {
    fn from(err: gimli::Error) -> Error {
        Error::Gimli(err)
    }
}
