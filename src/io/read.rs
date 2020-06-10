use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use chrono::{Duration, NaiveDate, NaiveDateTime};
use quick_xml::events::{BytesStart, Event};
use quick_xml::events::attributes::Attribute;
use zip::read::ZipFile;

use crate::{ColRange, Composit, CompositTag, CompositVec, RowRange, SCell, Sheet, StyleFor, StyleOrigin, StyleUse, ucell, Value, ValueFormat, ValueType, WorkBook};
use crate::error::OdsError;
use crate::format::{FormatPart, FormatPartType};
use crate::refs::{CellRef, parse_cellranges};
use crate::style::{FontDecl, HeaderFooter, PageLayout, Style};

/// Reads an ODS-file.
pub fn read_ods<P: AsRef<Path>>(path: P) -> Result<WorkBook, OdsError> {
    read_ods_flags(path, false)
}

/// Reads an ODS-file.
pub fn read_ods_flags<P: AsRef<Path>>(path: P, dump_xml: bool) -> Result<WorkBook, OdsError> {
    let file = File::open(path.as_ref())?;
    // ods is a zip-archive, we read content.xml
    let mut zip = zip::ZipArchive::new(file)?;

    let mut book = WorkBook::new();
    book.file = Some(path.as_ref().to_path_buf());

    read_content(&mut book, &mut zip.by_name("content.xml")?, dump_xml)?;
    read_styles(&mut book, &mut zip.by_name("styles.xml")?, dump_xml)?;

    Ok(book)
}

// Reads the content.xml
fn read_content(book: &mut WorkBook,
                zip_file: &mut ZipFile,
                dump_xml: bool) -> Result<(), OdsError> {
    // xml parser
    let mut xml = quick_xml::Reader::from_reader(BufReader::new(zip_file));
    xml.trim_text(true);

    let mut buf = Vec::new();

    let mut sheet = Sheet::new();

    // Separate counter for table-columns
    let mut tcol: ucell = 0;

    // Cell position
    let mut row: ucell = 0;
    let mut col: ucell = 0;

    // Rows can be repeated. In reality only empty ones ever are.
    let mut row_repeat: ucell = 1;
    // Row style.
    let mut row_style: Option<String> = None;

    let mut col_range_from = 0;
    let mut row_range_from = 0;

    loop {
        let event = xml.read_event(&mut buf)?;
        if dump_xml { println!("{:?}", event); }
        match event {
            Event::Start(xml_tag)
            if xml_tag.name() == b"table:table" => {
                read_table_attr(&xml, xml_tag, &mut sheet)?;
            }
            Event::End(xml_tag)
            if xml_tag.name() == b"table:table" => {
                row = 0;
                col = 0;
                book.push_sheet(sheet);
                sheet = Sheet::new();
            }

            Event::Start(xml_tag)
            if xml_tag.name() == b"table:table-header-columns" => {
                col_range_from = tcol;
            }

            Event::End(xml_tag)
            if xml_tag.name() == b"table:table-header-columns" => {
                sheet.header_cols = Some(ColRange::new(col_range_from, tcol - 1));
            }

            Event::Empty(xml_tag)
            if xml_tag.name() == b"table:table-column" => {
                tcol = read_table_column(&mut xml, &xml_tag, tcol, &mut sheet)?;
            }

            Event::Start(xml_tag)
            if xml_tag.name() == b"table:table-header-rows" => {
                row_range_from = row;
            }

            Event::End(xml_tag)
            if xml_tag.name() == b"table:table-header-rows" => {
                sheet.header_rows = Some(RowRange::new(row_range_from, row - 1));
            }

            Event::Start(xml_tag)
            if xml_tag.name() == b"table:table-row" => {
                row_repeat = read_table_row_attr(&mut xml, xml_tag, &mut row_style)?;
            }
            Event::End(xml_tag)
            if xml_tag.name() == b"table:table-row" => {
                // There is often a strange repeat count for the last
                // row of the table that is in the millions.
                // That hits the break quite thoroughly, for now I ignore
                // this. Removes the row style for empty rows, I can live
                // with that for now.
                //
                // if let Some(style) = row_style {
                //     for r in row..row + row_repeat {
                //         sheet.set_row_style(r, style.clone());
                //     }
                // }
                row_style = None;

                row += row_repeat;
                col = 0;
                row_repeat = 1;
            }

            Event::Start(xml_tag)
            if xml_tag.name() == b"office:font-face-decls" =>
                read_fonts(book, StyleOrigin::Content, &mut xml, dump_xml)?,

            Event::Start(xml_tag)
            if xml_tag.name() == b"office:styles" =>
                read_styles_tag(book, StyleOrigin::Content, &mut xml, dump_xml)?,

            Event::Start(xml_tag)
            if xml_tag.name() == b"office:automatic-styles" =>
                read_auto_styles(book, StyleOrigin::Content, &mut xml, dump_xml)?,

            Event::Start(xml_tag)
            if xml_tag.name() == b"office:master-styles" =>
                read_master_styles(book, StyleOrigin::Content, &mut xml, dump_xml)?,

            Event::Empty(xml_tag)
            if xml_tag.name() == b"table:table-cell" || xml_tag.name() == b"table:covered-table-cell" => {
                col = read_empty_table_cell(&mut sheet, &mut xml, xml_tag, row, col)?;
            }

            Event::Start(xml_tag)
            if xml_tag.name() == b"table:table-cell" || xml_tag.name() == b"table:covered-table-cell" => {
                col = read_table_cell(&mut sheet, &mut xml, xml_tag, row, col, dump_xml)?;
            }

            Event::Eof => {
                break;
            }
            _ => {}
        }

        buf.clear();
    }

    Ok(())
}

// Reads the table attributes.
fn read_table_attr(xml: &quick_xml::Reader<BufReader<&mut ZipFile>>,
                   xml_tag: BytesStart,
                   sheet: &mut Sheet) -> Result<(), OdsError> {
    for attr in xml_tag.attributes().with_checks(false) {
        match attr? {
            attr if attr.key == b"table:name" => {
                let v = attr.unescape_and_decode_value(xml)?;
                sheet.set_name(v);
            }
            attr if attr.key == b"table:style-name" => {
                let v = attr.unescape_and_decode_value(xml)?;
                sheet.set_style(v);
            }
            attr if attr.key == b"table:print-ranges" => {
                let v = attr.unescape_and_decode_value(xml)?;
                let mut pos = 0usize;
                sheet.print_ranges = parse_cellranges(v.as_str(), &mut pos)?;
            }
            _ => { /* ignore other attr */ }
        }
    }

    Ok(())
}

// Reads table-row attributes. Returns the repeat-count.
fn read_table_row_attr(xml: &mut quick_xml::Reader<BufReader<&mut ZipFile>>,
                       xml_tag: BytesStart,
                       row_style: &mut Option<String>) -> Result<ucell, OdsError>
{
    let mut row_repeat: ucell = 1;

    for attr in xml_tag.attributes().with_checks(false) {
        match attr? {
            attr if attr.key == b"table:number-rows-repeated" => {
                let v = attr.unescaped_value()?;
                let v = xml.decode(v.as_ref())?;
                row_repeat = v.parse::<ucell>()?;
            }
            attr if attr.key == b"table:style-name" => {
                let v = attr.unescape_and_decode_value(&xml)?;
                *row_style = Some(v);
            }
            _ => { /* ignore other */ }
        }
    }

    Ok(row_repeat)
}

// Reads the table-column attributes. Creates as many copies as indicated.
fn read_table_column(xml: &mut quick_xml::Reader<BufReader<&mut ZipFile>>,
                     xml_tag: &BytesStart,
                     mut tcol: ucell,
                     sheet: &mut Sheet) -> Result<ucell, OdsError> {
    let mut style = None;
    let mut cell_style = None;
    let mut repeat: ucell = 1;

    for attr in xml_tag.attributes().with_checks(false) {
        match attr? {
            attr if attr.key == b"table:style-name" => {
                let v = attr.unescape_and_decode_value(&xml)?;
                style = Some(v);
            }
            attr if attr.key == b"table:number-columns-repeated" => {
                let v = attr.unescape_and_decode_value(&xml)?;
                repeat = v.parse()?;
            }
            attr if attr.key == b"table:default-cell-style-name" => {
                let v = attr.unescape_and_decode_value(&xml)?;
                cell_style = Some(v);
            }
            _ => {}
        }
    }

    while repeat > 0 {
        if let Some(style) = &style {
            sheet.set_column_style(tcol, style.clone());
        }
        if let Some(cell_style) = &cell_style {
            sheet.set_column_cell_style(tcol, cell_style.clone());
        }
        tcol += 1;
        repeat -= 1;
    }

    Ok(tcol)
}

// Reads the cell data.
fn read_table_cell(sheet: &mut Sheet,
                   xml: &mut quick_xml::Reader<BufReader<&mut ZipFile>>,
                   xml_tag: BytesStart,
                   row: ucell,
                   mut col: ucell,
                   dump_xml: bool) -> Result<ucell, OdsError> {

    // Current cell tag
    let tag_name = xml_tag.name();

    // The current cell.
    let mut cell: SCell = SCell::new();
    // Columns can be repeated, not only empty ones.
    let mut cell_repeat: ucell = 1;
    // Decoded type.
    let mut value_type: Option<ValueType> = None;
    // Basic cell value here.
    let mut cell_value: Option<String> = None;
    // Content of the table-cell tag.
    let mut cell_content: Option<String> = None;
    // Currency
    let mut cell_currency: Option<String> = None;

    for attr in xml_tag.attributes().with_checks(false) {
        match attr? {
            attr if attr.key == b"table:number-columns-repeated" => {
                let v = attr.unescape_and_decode_value(&xml)?;
                cell_repeat = v.parse::<ucell>()?;
            }
            attr if attr.key == b"table:number-rows-spanned" => {
                let v = attr.unescape_and_decode_value(&xml)?;
                cell.span.0 = v.parse::<ucell>()?;
            }
            attr if attr.key == b"table:number-columns-spanned" => {
                let v = attr.unescape_and_decode_value(&xml)?;
                cell.span.1 = v.parse::<ucell>()?;
            }

            attr if attr.key == b"office:value-type" =>
                value_type = Some(decode_value_type(attr)?),

            attr if attr.key == b"office:date-value" =>
                cell_value = Some(attr.unescape_and_decode_value(&xml)?),
            attr if attr.key == b"office:time-value" =>
                cell_value = Some(attr.unescape_and_decode_value(&xml)?),
            attr if attr.key == b"office:value" =>
                cell_value = Some(attr.unescape_and_decode_value(&xml)?),
            attr if attr.key == b"office:boolean-value" =>
                cell_value = Some(attr.unescape_and_decode_value(&xml)?),

            attr if attr.key == b"office:currency" =>
                cell_currency = Some(attr.unescape_and_decode_value(&xml)?),

            attr if attr.key == b"table:formula" =>
                cell.formula = Some(attr.unescape_and_decode_value(&xml)?),
            attr if attr.key == b"table:style-name" =>
                cell.style = Some(attr.unescape_and_decode_value(&xml)?),

            _ => {}
        }
    }

    let mut buf = Vec::new();
    loop {
        let evt = xml.read_event(&mut buf)?;
        if dump_xml { println!(" style {:?}", evt); }
        match evt {
            // todo: insert CompositVec reading here
            Event::Text(xml_tag) => {
                // Not every cell type has a value attribute, some take
                // their value from the string representation.
                cell_content = text_append(cell_content, &xml_tag.unescape_and_decode(&xml)?);
            }

            Event::Start(xml_tag)
            if xml_tag.name() == b"text:p" => {
                cell_content = text_append_or(cell_content, "\n", "");
            }
            Event::Empty(xml_tag)
            if xml_tag.name() == b"text:p" => {}
            Event::End(xml_tag)
            if xml_tag.name() == b"text:p" => {}

            Event::Start(xml_tag)
            if xml_tag.name() == b"text:a" => {}
            Event::End(xml_tag)
            if xml_tag.name() == b"text:a" => {}

            Event::Empty(xml_tag)
            if xml_tag.name() == b"text:s" => {
                cell_content = text_append(cell_content, " ");
            }

            Event::End(xml_tag)
            if xml_tag.name() == tag_name => {
                cell.value = parse_value(value_type,
                                         cell_value,
                                         cell_content,
                                         cell_currency,
                                         row,
                                         col)?;

                while cell_repeat > 1 {
                    sheet.add_cell(row, col, cell.clone());
                    col += 1;
                    cell_repeat -= 1;
                }
                sheet.add_cell(row, col, cell);
                col += 1;

                break;
            }

            Event::Eof => {
                break;
            }

            _ => {}
        }

        buf.clear();
    }

    Ok(col)
}

/// For adding \n to the string. A leading \n is ignored.
fn text_append_or(text: Option<String>, append: &str, default: &str) -> Option<String> {
    match text {
        Some(s) => Some(s + append),
        None => Some(default.to_string())
    }
}

/// Append text.
fn text_append(text: Option<String>, append: &str) -> Option<String> {
    match text {
        Some(s) => Some(s + append),
        None => Some(append.to_owned())
    }
}

/// Reads a table-cell from an empty XML tag.
/// There seems to be no data associated, but it can have a style and a formula.
/// And first of all we need the repeat count for the correct placement.
fn read_empty_table_cell(sheet: &mut Sheet,
                         xml: &mut quick_xml::Reader<BufReader<&mut ZipFile>>,
                         xml_tag: BytesStart,
                         row: ucell,
                         mut col: ucell) -> Result<ucell, OdsError> {
    let mut cell = None;
    // Default advance is one column.
    let mut cell_repeat = 1;
    for attr in xml_tag.attributes().with_checks(false) {
        match attr? {
            attr if attr.key == b"table:number-columns-repeated" => {
                let v = attr.unescaped_value()?;
                let v = xml.decode(v.as_ref())?;
                cell_repeat = v.parse::<ucell>()?;
            }

            attr if attr.key == b"table:formula" => {
                if cell.is_none() {
                    cell = Some(SCell::new());
                }
                if let Some(c) = &mut cell {
                    c.formula = Some(attr.unescape_and_decode_value(&xml)?);
                }
            }
            attr if attr.key == b"table:style-name" => {
                if cell.is_none() {
                    cell = Some(SCell::new());
                }
                if let Some(c) = &mut cell {
                    c.style = Some(attr.unescape_and_decode_value(&xml)?);
                }
            }
            attr if attr.key == b"table:number-rows-spanned" => {
                if cell.is_none() {
                    cell = Some(SCell::new());
                }
                if let Some(c) = &mut cell {
                    let v = attr.unescape_and_decode_value(&xml)?;
                    c.span.0 = v.parse::<ucell>()?;
                }
            }
            attr if attr.key == b"table:number-columns-spanned" => {
                if cell.is_none() {
                    cell = Some(SCell::new());
                }
                if let Some(c) = &mut cell {
                    let v = attr.unescape_and_decode_value(&xml)?;
                    c.span.1 = v.parse::<ucell>()?;
                }
            }

            _ => { /* should be nothing else of interest here */ }
        }
    }

    if let Some(cell) = cell {
        while cell_repeat > 1 {
            sheet.add_cell(row, col, cell.clone());
            col += 1;
            cell_repeat -= 1;
        }
        sheet.add_cell(row, col, cell);
        col += 1;
    } else {
        col += cell_repeat;
    }

    Ok(col)
}

// Takes a bunch of strings and converts it to something useable.
fn parse_value(value_type: Option<ValueType>,
               cell_value: Option<String>,
               cell_content: Option<String>,
               cell_currency: Option<String>,
               row: ucell,
               col: ucell) -> Result<Value, OdsError> {
    if let Some(value_type) = value_type {
        match value_type {
            ValueType::Empty => {
                Ok(Value::Empty)
            }
            ValueType::Text => {
                if let Some(cell_content) = cell_content {
                    Ok(Value::Text(cell_content))
                } else {
                    Ok(Value::Text("".to_string()))
                }
            }
            ValueType::Number => {
                if let Some(cell_value) = cell_value {
                    let f = cell_value.parse::<f64>()?;
                    Ok(Value::Number(f))
                } else {
                    Err(OdsError::Ods(format!("{} has type number, but no value!", CellRef::simple(row, col))))
                }
            }
            ValueType::DateTime => {
                if let Some(cell_value) = cell_value {
                    let dt =
                        if cell_value.len() == 10 {
                            NaiveDate::parse_from_str(cell_value.as_str(), "%Y-%m-%d")?.and_hms(0, 0, 0)
                        } else {
                            NaiveDateTime::parse_from_str(cell_value.as_str(), "%Y-%m-%dT%H:%M:%S%.f")?
                        };

                    Ok(Value::DateTime(dt))
                } else {
                    Err(OdsError::Ods(format!("{} has type datetime, but no value!", CellRef::simple(row, col))))
                }
            }
            ValueType::TimeDuration => {
                if let Some(mut cell_value) = cell_value {
                    let mut hour: u32 = 0;
                    let mut have_hour = false;
                    let mut min: u32 = 0;
                    let mut have_min = false;
                    let mut sec: u32 = 0;
                    let mut have_sec = false;
                    let mut nanos: u32 = 0;
                    let mut nanos_digits: u8 = 0;

                    for c in cell_value.drain(..) {
                        match c {
                            'P' | 'T' => {}
                            '0'..='9' => {
                                if !have_hour {
                                    hour = hour * 10 + (c as u32 - '0' as u32);
                                } else if !have_min {
                                    min = min * 10 + (c as u32 - '0' as u32);
                                } else if !have_sec {
                                    sec = sec * 10 + (c as u32 - '0' as u32);
                                } else {
                                    nanos = nanos * 10 + (c as u32 - '0' as u32);
                                    nanos_digits += 1;
                                }
                            }
                            'H' => have_hour = true,
                            'M' => have_min = true,
                            '.' => have_sec = true,
                            'S' => {}
                            _ => {}
                        }
                    }
                    // unseen nano digits
                    while nanos_digits < 9 {
                        nanos *= 10;
                        nanos_digits += 1;
                    }

                    let secs: u64 = hour as u64 * 3600 + min as u64 * 60 + sec as u64;
                    let dur = Duration::from_std(std::time::Duration::new(secs, nanos))?;

                    Ok(Value::TimeDuration(dur))
                } else {
                    Err(OdsError::Ods(format!("{} has type time-duration, but no value!", CellRef::simple(row, col))))
                }
            }
            ValueType::Boolean => {
                if let Some(cell_value) = cell_value {
                    Ok(Value::Boolean(&cell_value == "true"))
                } else {
                    Err(OdsError::Ods(format!("{} has type boolean, but no value!", CellRef::simple(row, col))))
                }
            }
            ValueType::Currency => {
                if let Some(cell_value) = cell_value {
                    let f = cell_value.parse::<f64>()?;
                    if let Some(cell_currency) = cell_currency {
                        Ok(Value::Currency(cell_currency, f))
                    } else {
                        Err(OdsError::Ods(format!("{} has type currency, but no value!", CellRef::simple(row, col))))
                    }
                } else {
                    Err(OdsError::Ods(format!("{} has type currency, but no value!", CellRef::simple(row, col))))
                }
            }
            ValueType::Percentage => {
                if let Some(cell_value) = cell_value {
                    let f = cell_value.parse::<f64>()?;
                    Ok(Value::Percentage(f))
                } else {
                    Err(OdsError::Ods(format!("{} has type percentage, but no value!", CellRef::simple(row, col))))
                }
            }
        }
    } else {
        // could be an image or whatever
        Ok(Value::Empty)
    }
}

// String to ValueType
fn decode_value_type(attr: Attribute) -> Result<ValueType, OdsError> {
    match attr.unescaped_value()?.as_ref() {
        b"string" => Ok(ValueType::Text),
        b"float" => Ok(ValueType::Number),
        b"percentage" => Ok(ValueType::Percentage),
        b"date" => Ok(ValueType::DateTime),
        b"time" => Ok(ValueType::TimeDuration),
        b"boolean" => Ok(ValueType::Boolean),
        b"currency" => Ok(ValueType::Currency),
        other => Err(OdsError::Ods(format!("Unknown cell-type {:?}", other)))
    }
}

// reads a font-face
#[allow(clippy::single_match)]
fn read_fonts(book: &mut WorkBook,
              origin: StyleOrigin,
              xml: &mut quick_xml::Reader<BufReader<&mut ZipFile>>,
              dump_xml: bool) -> Result<(), OdsError> {
    let mut buf = Vec::new();

    let mut font: FontDecl = FontDecl::new_origin(origin);

    loop {
        let evt = xml.read_event(&mut buf)?;
        if dump_xml { println!(" style {:?}", evt); }
        match evt {
            Event::Start(ref xml_tag)
            | Event::Empty(ref xml_tag) => {
                match xml_tag.name() {
                    b"style:font-face" => {
                        for attr in xml_tag.attributes().with_checks(false) {
                            match attr? {
                                attr if attr.key == b"style:name" => {
                                    let v = attr.unescape_and_decode_value(&xml)?;
                                    font.set_name(v);
                                }
                                attr => {
                                    let k = xml.decode(&attr.key)?;
                                    let v = attr.unescape_and_decode_value(&xml)?;
                                    font.set_prp(k, v);
                                }
                            }
                        }

                        book.add_font(font);
                        font = FontDecl::new_origin(StyleOrigin::Content);
                    }
                    _ => {}
                }
            }

            Event::End(ref e) => {
                if e.name() == b"office:font-face-decls" {
                    break;
                }
            }

            Event::Eof => {
                break;
            }
            _ => {}
        }

        buf.clear();
    }

    Ok(())
}

// reads the page-layout tag
fn read_page_layout(book: &mut WorkBook,
                    xml: &mut quick_xml::Reader<BufReader<&mut ZipFile>>,
                    xml_tag: &BytesStart,
                    dump_xml: bool) -> Result<(), OdsError> {
    let mut buf = Vec::new();

    let mut pl = PageLayout::default();
    for attr in xml_tag.attributes().with_checks(false) {
        match attr? {
            attr if attr.key == b"style:name" => {
                let v = attr.unescape_and_decode_value(&xml)?;
                pl.set_name(v);
            }
            _ => (),
        }
    }

    let mut header_style = false;
    let mut footer_style = false;

    loop {
        let evt = xml.read_event(&mut buf)?;
        if dump_xml { println!(" page-layout {:?}", evt); }
        match evt {
            Event::Start(ref xml_tag)
            | Event::Empty(ref xml_tag) => {
                match xml_tag.name() {
                    b"style:page-layout-properties" =>
                        copy_pagelayout_properties(&mut pl, &PageLayout::set_prp, xml, xml_tag)?,
                    b"style:header-style" =>
                        header_style = true,
                    b"style:footer-style" =>
                        footer_style = true,
                    b"style:header-footer-properties" => {
                        if header_style {
                            copy_pagelayout_properties(&mut pl, &PageLayout::set_header_prp, xml, xml_tag)?;
                        }
                        if footer_style {
                            copy_pagelayout_properties(&mut pl, &PageLayout::set_footer_prp, xml, xml_tag)?;
                        }
                    }
                    _ => (),
                }
            }
            Event::Text(_) => (),
            Event::End(ref end) => {
                match end.name() {
                    b"style:page-layout" =>
                        break,
                    b"style:header-style" =>
                        header_style = false,
                    b"style:footer-style" =>
                        footer_style = false,
                    _ => (),
                }
            }
            Event::Eof => break,
            _ => (),
        }

        buf.clear();
    }

    book.add_pagelayout(pl);

    Ok(())
}

// copy all attr of the xml_tag. uses the given function for the setter.
fn copy_pagelayout_properties(pagelayout: &mut PageLayout,
                              add_fn: &dyn Fn(&mut PageLayout, &str, String),
                              xml: &mut quick_xml::Reader<BufReader<&mut ZipFile>>,
                              xml_tag: &BytesStart) -> Result<(), OdsError> {
    for attr in xml_tag.attributes().with_checks(false) {
        if let Ok(attr) = attr {
            let k = xml.decode(&attr.key)?;
            let v = attr.unescape_and_decode_value(&xml)?;
            add_fn(pagelayout, k, v);
        }
    }

    Ok(())
}

// read the master-styles tag
fn read_master_styles(book: &mut WorkBook,
                      origin: StyleOrigin,
                      xml: &mut quick_xml::Reader<BufReader<&mut ZipFile>>,
                      dump_xml: bool) -> Result<(), OdsError> {
    let mut buf = Vec::new();

    loop {
        let evt = xml.read_event(&mut buf)?;
        if dump_xml { println!(" master-styles {:?}", evt); }
        match evt {
            Event::Start(ref xml_tag)
            | Event::Empty(ref xml_tag) => {
                match xml_tag.name() {
                    b"style:master-page" => {
                        read_master_page(book, origin, xml, xml_tag, dump_xml)?;
                    }
                    _ => (),
                }
            }
            Event::Text(_) => (),
            Event::End(ref e) => {
                if e.name() == b"office:master-styles" {
                    break;
                }
            }
            Event::Eof => break,
            _ => (),
        }

        buf.clear();
    }

    Ok(())
}

// read the master-page tag
fn read_master_page(book: &mut WorkBook,
                    _origin: StyleOrigin,
                    xml: &mut quick_xml::Reader<BufReader<&mut ZipFile>>,
                    xml_tag: &BytesStart,
                    dump_xml: bool) -> Result<(), OdsError> {
    let mut buf = Vec::new();

    let mut masterpage_name = "".to_string();
    let mut pagelayout_name = "".to_string();
    for attr in xml_tag.attributes().with_checks(false) {
        match attr? {
            attr if attr.key == b"style:name" => {
                masterpage_name = attr.unescape_and_decode_value(&xml)?;
            }
            attr if attr.key == b"style:page-layout-name" => {
                pagelayout_name = attr.unescape_and_decode_value(&xml)?;
            }
            _ => (),
        }
    }

    // may not exist? but should
    if book.pagelayout(&pagelayout_name).is_none() {
        let mut p = PageLayout::default();
        p.set_name(pagelayout_name.clone());
        book.add_pagelayout(p);
    }

    let pl = book.pagelayout_mut(&pagelayout_name).unwrap();
    pl.set_masterpage_name(masterpage_name);

    loop {
        let evt = xml.read_event(&mut buf)?;
        //let empty_tag = if let Event::Empty(_) = evt { true } else { false };
        if dump_xml { println!(" master-page {:?}", evt); }
        match evt {
            Event::Start(ref xml_tag) |
            Event::Empty(ref xml_tag) => {
                match xml_tag.name() {
                    b"style:header" => {
                        let hf = read_headerfooter(b"style:header", xml, dump_xml)?;
                        pl.set_header(hf);
                    }
                    b"style:header-left" => {
                        let hf = read_headerfooter(b"style:header", xml, dump_xml)?;
                        pl.set_header_left(hf);
                    }
                    b"style:footer" => {
                        let hf = read_headerfooter(b"style:header", xml, dump_xml)?;
                        pl.set_footer(hf);
                    }
                    b"style:footer-left" => {
                        let hf = read_headerfooter(b"style:header", xml, dump_xml)?;
                        pl.set_footer_left(hf);
                    }
                    _ => (),
                }
            }

            Event::Text(_) => (),
            Event::End(ref e) => {
                if e.name() == b"style:master-page" {
                    break;
                }
            }
            Event::Eof => break,
            _ => (),
        }

        buf.clear();
    }

    Ok(())
}

// reads any header or footer tags
fn read_headerfooter(end_tag: &[u8],
                     xml: &mut quick_xml::Reader<BufReader<&mut ZipFile>>,
                     dump_xml: bool) -> Result<HeaderFooter, OdsError> {
    let mut buf = Vec::new();

    let mut hf = HeaderFooter::new();

    loop {
        let evt = xml.read_event(&mut buf)?;
        //let empty_tag = if let Event::Empty(_) = evt { true } else { false };
        if dump_xml { println!(" style {:?}", evt); }
        match evt {
            Event::Start(ref xml_tag) |
            Event::Empty(ref xml_tag) => {
                match xml_tag.name() {
                    b"style:region-left" => {
                        let cm = read_composit(b"style:region-left", xml, dump_xml)?;
                        hf.set_region_left(cm);
                    }
                    b"style:region-center" => {
                        let cm = read_composit(b"style:region-left", xml, dump_xml)?;
                        hf.set_region_center(cm);
                    }
                    b"style:region-right" => {
                        let cm = read_composit(b"style:region-left", xml, dump_xml)?;
                        hf.set_region_right(cm);
                    }
                    b"text:p" => {
                        let cm = read_composit(b"text:p", xml, dump_xml)?;
                        hf.set_content(cm);
                    }
                    // TODO: other than text:p maybe?
                    _ => (),
                }
            }
            Event::Text(_) => (),
            Event::End(ref e) => {
                if e.name() == end_tag {
                    break;
                }
            }
            Event::Eof => break,
            _ => (),
        }

        buf.clear();
    }

    Ok(hf)
}

// reads all the tags up to end_tag and creates a CompositVec
fn read_composit(end_tag: &[u8],
                 xml: &mut quick_xml::Reader<BufReader<&mut ZipFile>>,
                 dump_xml: bool) -> Result<CompositVec, OdsError> {
    let mut buf = Vec::new();

    let mut cv = CompositVec::new();
    loop {
        let evt = xml.read_event(&mut buf)?;
        //let empty_tag = if let Event::Empty(_) = evt { true } else { false };
        if dump_xml { println!(" master-page {:?}", evt); }
        match evt {
            Event::Start(ref xml_tag) => {
                let n = xml.decode(xml_tag.name())?;
                let mut c = CompositTag::new(n);

                for attr in xml_tag.attributes().with_checks(false) {
                    if let Ok(attr) = attr {
                        let k = xml.decode(attr.key)?;
                        let v = attr.unescape_and_decode_value(xml)?;
                        c.set_attr(k, v);
                    }
                }
                cv.push(Composit::Start(c));
            }
            Event::Empty(ref xml_tag) => {
                let n = xml.decode(xml_tag.name())?;
                let mut c = CompositTag::new(n);

                for attr in xml_tag.attributes().with_checks(false) {
                    if let Ok(attr) = attr {
                        let k = xml.decode(attr.key)?;
                        let v = attr.unescape_and_decode_value(xml)?;
                        c.set_attr(k, v);
                    }
                }
                cv.push(Composit::Empty(c));
            }
            Event::Text(ref txt) => {
                let txt = txt.unescape_and_decode(xml)?;
                cv.push(Composit::Text(txt));
            }
            Event::End(ref xml_tag) => {
                if xml_tag.name() == end_tag {
                    break;
                } else {
                    let n = xml.decode(xml_tag.name())?;
                    cv.push(Composit::End(n.to_string()));
                }
            }
            Event::Eof => break,
            _ => (),
        }

        buf.clear();
    }

    Ok(cv)
}

// reads the office-styles tag
fn read_styles_tag(book: &mut WorkBook,
                   origin: StyleOrigin,
                   xml: &mut quick_xml::Reader<BufReader<&mut ZipFile>>,
                   dump_xml: bool) -> Result<(), OdsError> {
    let mut buf = Vec::new();

    loop {
        let evt = xml.read_event(&mut buf)?;
        let empty_tag = if let Event::Empty(_) = evt { true } else { false };
        if dump_xml { println!(" style {:?}", evt); }
        match evt {
            Event::Start(ref xml_tag) |
            Event::Empty(ref xml_tag) => {
                match xml_tag.name() {
                    b"style:style" => {
                        read_style_style(book, origin, StyleUse::Named, xml, xml_tag, empty_tag, dump_xml)?;
                    }
                    // style:default-style
                    b"number:boolean-style" |
                    b"number:date-style" |
                    b"number:time-style" |
                    b"number:number-style" |
                    b"number:currency-style" |
                    b"number:percentage-style" |
                    b"number:text-style" => {
                        read_value_format(book, origin, StyleUse::Named, xml, xml_tag, dump_xml)?;
                    }
                    // style:default-page-layout
                    _ => (),
                }
            }
            Event::Text(_) => (),
            Event::End(ref e) => {
                if e.name() == b"office:styles" {
                    break;
                }
            }
            Event::Eof => break,
            _ => (),
        }

        buf.clear();
    }

    Ok(())
}

// read the automatic-styles tag
fn read_auto_styles(book: &mut WorkBook,
                    origin: StyleOrigin,
                    xml: &mut quick_xml::Reader<BufReader<&mut ZipFile>>,
                    dump_xml: bool) -> Result<(), OdsError> {
    let mut buf = Vec::new();

    loop {
        let evt = xml.read_event(&mut buf)?;
        let empty_tag = if let Event::Empty(_) = evt { true } else { false };
        if dump_xml { println!(" automatic-styles {:?}", evt); }
        match evt {
            Event::Start(ref xml_tag)
            | Event::Empty(ref xml_tag) => {
                match xml_tag.name() {
                    b"style:style" => {
                        read_style_style(book, origin, StyleUse::Automatic, xml, xml_tag, empty_tag, dump_xml)?;
                    }
                    // style:default-style
                    b"number:boolean-style" |
                    b"number:date-style" |
                    b"number:time-style" |
                    b"number:number-style" |
                    b"number:currency-style" |
                    b"number:percentage-style" |
                    b"number:text-style" => {
                        read_value_format(book, origin, StyleUse::Automatic, xml, xml_tag, dump_xml)?;
                    }
                    // style:default-page-layout
                    b"style:page-layout" => {
                        read_page_layout(book, xml, xml_tag, dump_xml)?;
                    }
                    _ => (),
                }
            }
            Event::Text(_) => (),
            Event::End(ref e) => {
                if e.name() == b"office:automatic-styles" {
                    break;
                }
            }
            Event::Eof => break,
            _ => (),
        }

        buf.clear();
    }

    Ok(())
}

// Reads any of the number:xxx tags
fn read_value_format(book: &mut WorkBook,
                     origin: StyleOrigin,
                     styleuse: StyleUse,
                     xml: &mut quick_xml::Reader<BufReader<&mut ZipFile>>,
                     xml_tag: &BytesStart,
                     dump_xml: bool) -> Result<(), OdsError> {
    let mut buf = Vec::new();

    let mut value_style = ValueFormat::new_origin(origin, styleuse);
    // Styles with content information are stored before completion.
    let mut value_style_part = None;

    match xml_tag.name() {
        b"number:boolean-style" =>
            read_value_format_attr(ValueType::Boolean, &mut value_style, xml, xml_tag)?,
        b"number:date-style" =>
            read_value_format_attr(ValueType::DateTime, &mut value_style, xml, xml_tag)?,
        b"number:time-style" =>
            read_value_format_attr(ValueType::TimeDuration, &mut value_style, xml, xml_tag)?,
        b"number:number-style" =>
            read_value_format_attr(ValueType::Number, &mut value_style, xml, xml_tag)?,
        b"number:currency-style" =>
            read_value_format_attr(ValueType::Currency, &mut value_style, xml, xml_tag)?,
        b"number:percentage-style" =>
            read_value_format_attr(ValueType::Percentage, &mut value_style, xml, xml_tag)?,
        b"number:text-style" =>
            read_value_format_attr(ValueType::Text, &mut value_style, xml, xml_tag)?,
        _ => (),
    }

    loop {
        let evt = xml.read_event(&mut buf)?;
        if dump_xml { println!(" style {:?}", evt); }
        match evt {
            Event::Start(ref xml_tag)
            | Event::Empty(ref xml_tag) => {
                match xml_tag.name() {
                    b"number:boolean" =>
                        push_value_format_part(&mut value_style, FormatPartType::Boolean, xml, xml_tag)?,
                    b"number:number" =>
                        push_value_format_part(&mut value_style, FormatPartType::Number, xml, xml_tag)?,
                    b"number:scientific-number" =>
                        push_value_format_part(&mut value_style, FormatPartType::Scientific, xml, xml_tag)?,
                    b"number:day" =>
                        push_value_format_part(&mut value_style, FormatPartType::Day, xml, xml_tag)?,
                    b"number:month" =>
                        push_value_format_part(&mut value_style, FormatPartType::Month, xml, xml_tag)?,
                    b"number:year" =>
                        push_value_format_part(&mut value_style, FormatPartType::Year, xml, xml_tag)?,
                    b"number:era" =>
                        push_value_format_part(&mut value_style, FormatPartType::Era, xml, xml_tag)?,
                    b"number:day-of-week" =>
                        push_value_format_part(&mut value_style, FormatPartType::DayOfWeek, xml, xml_tag)?,
                    b"number:week-of-year" =>
                        push_value_format_part(&mut value_style, FormatPartType::WeekOfYear, xml, xml_tag)?,
                    b"number:quarter" =>
                        push_value_format_part(&mut value_style, FormatPartType::Quarter, xml, xml_tag)?,
                    b"number:hours" =>
                        push_value_format_part(&mut value_style, FormatPartType::Hours, xml, xml_tag)?,
                    b"number:minutes" =>
                        push_value_format_part(&mut value_style, FormatPartType::Minutes, xml, xml_tag)?,
                    b"number:seconds" =>
                        push_value_format_part(&mut value_style, FormatPartType::Seconds, xml, xml_tag)?,
                    b"number:fraction" =>
                        push_value_format_part(&mut value_style, FormatPartType::Fraction, xml, xml_tag)?,
                    b"number:am-pm" =>
                        push_value_format_part(&mut value_style, FormatPartType::AmPm, xml, xml_tag)?,
                    b"number:embedded-text" =>
                        push_value_format_part(&mut value_style, FormatPartType::EmbeddedText, xml, xml_tag)?,
                    b"number:text-content" =>
                        push_value_format_part(&mut value_style, FormatPartType::TextContent, xml, xml_tag)?,
                    b"style:text" =>
                        push_value_format_part(&mut value_style, FormatPartType::Day, xml, xml_tag)?,
                    b"style:map" =>
                        push_value_format_part(&mut value_style, FormatPartType::StyleMap, xml, xml_tag)?,
                    b"number:currency-symbol" => {
                        value_style_part = Some(read_part(xml, xml_tag, FormatPartType::CurrencySymbol)?);

                        // Empty-Tag. Finish here.
                        if let Event::Empty(_) = evt {
                            if let Some(part) = value_style_part {
                                value_style.push_part(part);
                            }
                            value_style_part = None;
                        }
                    }
                    b"number:text" => {
                        value_style_part = Some(read_part(xml, xml_tag, FormatPartType::Text)?);

                        // Empty-Tag. Finish here.
                        if let Event::Empty(_) = evt {
                            if let Some(part) = value_style_part {
                                value_style.push_part(part);
                            }
                            value_style_part = None;
                        }
                    }
                    _ => (),
                }
            }
            Event::Text(ref e) => {
                if let Some(part) = &mut value_style_part {
                    part.content = Some(e.unescape_and_decode(&xml)?);
                }
            }
            Event::End(ref e) => {
                match e.name() {
                    b"number:boolean-style" |
                    b"number:date-style" |
                    b"number:time-style" |
                    b"number:number-style" |
                    b"number:currency-style" |
                    b"number:percentage-style" |
                    b"number:text-style" => {
                        book.add_format(value_style);
                        break;
                    }
                    b"number:currency-symbol" | b"number:text" => {
                        if let Some(part) = value_style_part {
                            value_style.push_part(part);
                        }
                        value_style_part = None;
                    }
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => (),
        }

        buf.clear();
    }

    Ok(())
}

/// Copies all the attr from the tag.
fn read_value_format_attr(value_type: ValueType,
                          value_style: &mut ValueFormat,
                          xml: &mut quick_xml::Reader<BufReader<&mut ZipFile>>,
                          xml_tag: &BytesStart) -> Result<(), OdsError> {
    value_style.v_type = value_type;

    for attr in xml_tag.attributes().with_checks(false) {
        match attr? {
            attr if attr.key == b"style:name" => {
                let v = attr.unescape_and_decode_value(&xml)?;
                value_style.set_name(v);
            }
            attr => {
                let k = xml.decode(&attr.key)?;
                let v = attr.unescape_and_decode_value(&xml)?;
                value_style.set_prp(k, v);
            }
        }
    }

    Ok(())
}

/// Append a format-part tag
fn push_value_format_part(value_style: &mut ValueFormat,
                          part_type: FormatPartType,
                          xml: &mut quick_xml::Reader<BufReader<&mut ZipFile>>,
                          xml_tag: &BytesStart) -> Result<(), OdsError> {
    value_style.push_part(read_part(xml, xml_tag, part_type)?);

    Ok(())
}

fn read_part(xml: &mut quick_xml::Reader<BufReader<&mut ZipFile>>,
             xml_tag: &BytesStart,
             part_type: FormatPartType) -> Result<FormatPart, OdsError> {
    let mut part = FormatPart::new(part_type);

    for a in xml_tag.attributes().with_checks(false) {
        if let Ok(attr) = a {
            let k = xml.decode(&attr.key)?;
            let v = attr.unescape_and_decode_value(&xml)?;

            part.set_prp(k, v);
        }
    }

    Ok(part)
}

// style:style tag
fn read_style_style(book: &mut WorkBook,
                    origin: StyleOrigin,
                    styleuse: StyleUse,
                    xml: &mut quick_xml::Reader<BufReader<&mut ZipFile>>,
                    xml_tag: &BytesStart,
                    empty_tag: bool,
                    dump_xml: bool) -> Result<(), OdsError> {
    let mut buf = Vec::new();
    let mut style: Style = Style::new_origin(origin, styleuse);

    read_style_attr(xml, xml_tag, &mut style)?;

    // In case of an empty xml-tag we are done here.
    if empty_tag {
        book.add_style(style);
    } else {
        loop {
            let evt = xml.read_event(&mut buf)?;
            if dump_xml { println!(" style {:?}", evt); }
            match evt {
                Event::Start(ref xml_tag)
                | Event::Empty(ref xml_tag) => {
                    match xml_tag.name() {
// style:chart-properties
// style:drawing-page-properties
// style:graphic-properties
// style:ruby-properties
// style:section-properties
                        b"style:table-properties" =>
                            copy_style_properties(&mut style, &Style::set_table_prp, xml, xml_tag)?,
                        b"style:table-column-properties" =>
                            copy_style_properties(&mut style, &Style::set_table_col_prp, xml, xml_tag)?,
                        b"style:table-row-properties" =>
                            copy_style_properties(&mut style, &Style::set_table_row_prp, xml, xml_tag)?,
                        b"style:table-cell-properties" =>
                            copy_style_properties(&mut style, &Style::set_table_cell_prp, xml, xml_tag)?,
                        b"style:text-properties" =>
                            copy_style_properties(&mut style, &Style::set_text_prp, xml, xml_tag)?,
                        b"style:paragraph-properties" =>
                            copy_style_properties(&mut style, &Style::set_paragraph_prp, xml, xml_tag)?,
                        _ => (),
                    }
                }
                Event::Text(_) => (),
                Event::End(ref e) => {
                    match e.name() {
                        b"style:style" => {
                            book.add_style(style);
                            break;
                        }
                        _ => (),
                    }
                }
                Event::Eof => break,
                _ => (),
            }
        }
    }

    Ok(())
}

fn read_style_attr(xml: &mut quick_xml::Reader<BufReader<&mut ZipFile>>,
                   xml_tag: &BytesStart,
                   style: &mut Style) -> Result<(), OdsError> {
    for attr in xml_tag.attributes().with_checks(false) {
        match attr? {
            attr if attr.key == b"style:name" => {
                let v = attr.unescape_and_decode_value(&xml)?;
                style.set_name(v);
            }
            attr if attr.key == b"style:family" => {
                let v = attr.unescape_and_decode_value(&xml)?;
                match v.as_ref() {
                    "table" => style.family = StyleFor::Table,
                    "table-column" => style.family = StyleFor::TableColumn,
                    "table-row" => style.family = StyleFor::TableRow,
                    "table-cell" => style.family = StyleFor::TableCell,
                    _ => {}
                }
            }
            attr if attr.key == b"style:parent-style-name" => {
                let v = attr.unescape_and_decode_value(&xml)?;
                style.parent = Some(v);
            }
            attr if attr.key == b"style:data-style-name" => {
                let v = attr.unescape_and_decode_value(&xml)?;
                style.value_format = Some(v);
            }
            _ => { /* noop */ }
        }
    }

    Ok(())
}

fn copy_style_properties(style: &mut Style,
                         add_fn: &dyn Fn(&mut Style, &str, String),
                         xml: &mut quick_xml::Reader<BufReader<&mut ZipFile>>,
                         xml_tag: &BytesStart) -> Result<(), OdsError> {
    for attr in xml_tag.attributes().with_checks(false) {
        if let Ok(attr) = attr {
            let k = xml.decode(&attr.key)?;
            let v = attr.unescape_and_decode_value(&xml)?;
            add_fn(style, k, v);
        }
    }

    Ok(())
}


fn read_styles(book: &mut WorkBook,
               zip_file: &mut ZipFile,
               dump_xml: bool) -> Result<(), OdsError> {
    let mut xml = quick_xml::Reader::from_reader(BufReader::new(zip_file));
    xml.trim_text(true);

    let mut buf = Vec::new();
    loop {
        let event = xml.read_event(&mut buf)?;
        if dump_xml { println!("{:?}", event); }
        match event {
            Event::Start(xml_tag)
            if xml_tag.name() == b"office:font-face-decls" =>
                read_fonts(book, StyleOrigin::Styles, &mut xml, dump_xml)?,
            Event::Start(xml_tag)
            if xml_tag.name() == b"office:styles" =>
                read_styles_tag(book, StyleOrigin::Styles, &mut xml, dump_xml)?,
            Event::Start(xml_tag)
            if xml_tag.name() == b"office:automatic-styles" =>
                read_auto_styles(book, StyleOrigin::Styles, &mut xml, dump_xml)?,
            Event::Start(xml_tag)
            if xml_tag.name() == b"office:master-styles" =>
                read_master_styles(book, StyleOrigin::Styles, &mut xml, dump_xml)?,
            Event::Eof => {
                break;
            }
            _ => {}
        }

        buf.clear();
    }

    Ok(())
}


