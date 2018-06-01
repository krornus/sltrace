extern crate addr2line;
extern crate fallible_iterator;
extern crate gimli;
extern crate memmap;
extern crate object;
extern crate rdb;
extern crate x86asm;

use std::fs::File;
use std::io::{self, Seek, SeekFrom};
use std::ops::Range;

use fallible_iterator::FallibleIterator;
use object::Object;
use rdb::debugger::Debugger;
use x86asm::{InstructionReader, Mode};

/* i dont wanna deal with types anymore */
macro_rules! buf2str {
    ($buf:expr) => {
        if let Some(s) = $buf {
            match s.to_string() {
                Ok(s) => s,
                _ => "",
            }
        } else {
            ""
        }
    };
}

fn main() -> io::Result<()> {
    let dbg = Debugger::new(
        "/home/spowell/programming/c/feh/src/feh",
        vec![
            "--bg-center",
            "/home/spowell/pictures/backgrounds/darkstar_poster.jpg",
        ],
    );

    let addr = 0x155c3;

    let mut file = File::open("/home/spowell/programming/c/feh/src/feh").unwrap();

    let reader_file = file.try_clone().expect("failed to clone file handler");

    let mut reader = InstructionReader::new(reader_file, Mode::Real);

    file.seek(SeekFrom::Start(addr))?;
    let ins = reader.read();

    run().expect("failed to run");

    Ok(())
}

fn run() -> Result<(), Error> {
    let file = File::open("/home/spowell/programming/c/feh/src/feh").unwrap();
    let map = unsafe { memmap::Mmap::map(&file).unwrap() };
    let obj = object::File::parse(&*map).unwrap();

    let debug_line: gimli::DebugLine<_> = load_section(&obj)?;

    let unit_wrapper = load_units(&obj)?;
    let units = &unit_wrapper.units;

    for unit in units.iter() {
        if let Some(AddressRange::Range(ref r)) = unit.ranges {
            println!(
                "0x{:x},0x{:x} -> entry: {}",
                r.start,
                r.end,
                0x6230 >= r.start && 0x6230 < r.end
            );
        }

        let program = debug_line.program(
            unit.line_offset.unwrap(),
            unit.address_size,
            unit.comp_dir,
            unit.name,
        )?;

        let (program, sequences) = program.sequences().unwrap();
        for sequence in &sequences {
            let mut sm = program.resume_from(sequence);

            let dir = buf2str!(unit.comp_dir);
            let name = buf2str!(unit.name);

            loop {
                match sm.next_row() {
                    Ok(Some((header, row))) => {
                        //println!("[+] {}/{} {}:{:?}",
                        //    dir,
                        //    name,
                        //    row.line().unwrap_or(0),
                        //    row.column()
                        //);
                    }
                    Ok(None) => {
                        break;
                    }
                    _ => {
                        continue;
                    }
                }
            }
        }
    }

    Ok(())
}

#[derive(Debug)]
enum AddressRange<R>
where
    R: gimli::Reader,
{
    Range(Range<u64>),
    GimliRange(gimli::RangesIter<R>),
}

struct CompilationUnits<'input, R, Endian, T = usize>
where
    R: gimli::Reader,
    Endian: gimli::Endianity + 'input,
    T: 'input,
{
    units: Vec<UnitData<'input, R, Endian, T>>,
}

struct UnitData<'input, R, Endian, T = usize>
where
    R: gimli::Reader,
    Endian: gimli::Endianity,
{
    unit: gimli::CompilationUnitHeader<gimli::EndianBuf<'input, gimli::RunTimeEndian>, usize>,
    ranges: Option<AddressRange<R>>,
    abbreviations: gimli::Abbreviations,
    name: Option<gimli::EndianBuf<'input, Endian>>,
    comp_dir: Option<gimli::EndianBuf<'input, Endian>>,
    tag: gimli::DwTag,
    low_pc: Option<u64>,
    address_size: u8,
    line_offset: Option<gimli::DebugLineOffset<T>>,
}

fn load_range<R: gimli::Reader>(
    entry: &gimli::DebuggingInformationEntry<R, R::Offset>,
    debug_ranges: &gimli::DebugRanges<R>,
    size: u8,
    low: u64,
    base_address: u64,
) -> Result<AddressRange<R>, Error> {
    entry.attr_value(gimli::DW_AT_ranges)?.map_or_else(
        || match entry.attr_value(gimli::DW_AT_high_pc)? {
            Some(gimli::AttributeValue::Addr(addr)) => Ok(AddressRange::Range(low..addr)),
            Some(gimli::AttributeValue::Udata(x)) => Ok(AddressRange::Range(low..low + x)),
            None => Err("No high end for address range".into()),
            _ => Err("Invalid attribute value for ranges".into()),
        },
        |r| {
            if let gimli::AttributeValue::DebugRangesRef(offset) = r {
                Ok(AddressRange::GimliRange(debug_ranges.ranges(
                    offset,
                    size,
                    base_address,
                )?))
            } else {
                Err("Invalid attribute value for ranges".into())
            }
        },
    )
}

fn load_units<'a>(
    obj: &object::File<'a>,
) -> Result<
    CompilationUnits<'a, gimli::EndianBuf<'a, gimli::RunTimeEndian>, gimli::RunTimeEndian, usize>,
    Error,
> {
    let debug_info: gimli::DebugInfo<_> = load_section(&obj)?;
    let debug_abbrev: gimli::DebugAbbrev<_> = load_section(&obj)?;
    let debug_str: gimli::DebugStr<_> = load_section(&obj)?;
    let debug_ranges: gimli::DebugRanges<_> = load_section(&obj)?;

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
                let (mut name, mut comp_dir): (
                    Option<gimli::EndianBuf<_>>,
                    Option<gimli::EndianBuf<_>>,
                );
                let (mut low, mut size): (Option<u64>, u8);
                let mut tag: gimli::DwTag;
                let mut dbg_line: Option<gimli::DebugLineOffset<_>> = None;
                let mut ranges: Option<AddressRange<_>> = None;

                let abbrevs: gimli::Abbreviations = unit.abbreviations(&debug_abbrev)?;
                {
                    let mut cursor = unit.entries(&abbrevs);
                    let (_, entry) = cursor.next_dfs()?.unwrap();

                    size = unit.address_size();
                    tag = entry.tag();

                    if tag != gimli::DW_TAG_compile_unit {
                        continue;
                    }

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

                    if let Some(low) = low {
                        ranges = load_range(entry, &debug_ranges, size, low, low).ok();
                    }

                    idx += 1;
                }

                let udata = UnitData {
                    ranges: ranges,
                    unit: unit,
                    abbreviations: abbrevs,
                    name,
                    comp_dir,
                    tag,
                    low_pc: low,
                    address_size: size,
                    line_offset: dbg_line,
                };

                units.push(udata);
            }
            Ok(None) => {
                break;
            }
            _ => {
                continue;
            }
        }
    }

    let units = Ok(CompilationUnits { units: units });

    units
}

fn load_section<'input, 'file, Section>(file: &object::File<'input>) -> Result<Section, Error>
where
    'input: 'file,
    Section: gimli::Section<gimli::EndianBuf<'input, gimli::RunTimeEndian>>,
{
    let endian = file.endianness();
    let name = Section::section_name();
    let buf = file
        .section_data_by_name(name)
        .ok_or(Error::MissingSection)?;
    let reader = gimli::EndianBuf::new(&buf, endian);

    Ok(Section::from(reader))
}

trait Endianness {
    fn endianness(&self) -> gimli::RunTimeEndian;
}

impl<'data, 'file, Object> Endianness for Object
where
    'data: 'file,
    Object: object::Object<'data, 'file>,
{
    fn endianness(&self) -> gimli::RunTimeEndian {
        match self.is_little_endian() {
            true => gimli::RunTimeEndian::Little,
            false => gimli::RunTimeEndian::Big,
        }
    }
}

#[derive(Debug)]
enum Error {
    MissingSection,
    Gimli(gimli::Error),
    Message(String),
}

impl From<String> for Error {
    fn from(err: String) -> Error {
        Error::Message(err)
    }
}

impl<'a> From<&'a str> for Error {
    fn from(err: &str) -> Error {
        Error::Message(err.to_string())
    }
}

impl From<gimli::Error> for Error {
    fn from(err: gimli::Error) -> Error {
        Error::Gimli(err)
    }
}
