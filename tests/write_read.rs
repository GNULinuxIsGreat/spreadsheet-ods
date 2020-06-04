use spreadsheet_ods::{Sheet, WorkBook};
use spreadsheet_ods::ods::{OdsError, read_ods, read_ods_flags, write_ods};

#[test]
fn test_0() -> Result<(), OdsError> {
    let mut wb = WorkBook::new();
    let mut sh = Sheet::new();

    sh.set_value(0, 0, "A");

    wb.push_sheet(sh);

    write_ods(&wb, "test_out/test_0.ods")?;

    let wi = read_ods("test_out/test_0.ods")?;
    let si = wi.sheet(0);

    assert_eq!(si.value(0, 0).as_str_or(""), "A");

    Ok(())
}

#[test]
fn test_span() -> Result<(), OdsError> {
    env_logger::init();

    let mut wb = WorkBook::new();

    let mut sh = Sheet::new();
    sh.set_value(0, 0, "A");
    sh.set_value(0, 1, "A2");
    sh.set_value(0, 2, "bomb");
    sh.set_value(1, 0, "bomb");
    sh.set_value(1, 1, "bomb");
    sh.set_value(1, 2, "bomb");
    if let Some(c) = sh.cell_mut(0, 0) {
        c.set_col_span(2);
    }
    wb.push_sheet(sh);

    let mut sh = Sheet::new();
    sh.set_value(1, 0, "B");
    sh.set_value(2, 0, "B2");
    sh.set_value(1, 1, "bomb");
    sh.set_value(2, 1, "bomb");
    sh.set_value(3, 0, "bomb");
    sh.set_value(3, 1, "bomb");
    if let Some(c) = sh.cell_mut(1, 0) {
        c.set_row_span(2);
    }
    wb.push_sheet(sh);

    let mut sh = Sheet::new();
    sh.set_value(3, 0, "C");
    sh.set_value(3, 1, "C2");
    sh.set_value(4, 0, "C2");
    sh.set_value(4, 1, "C2");
    sh.set_value(3, 2, "bomb");
    sh.set_value(4, 2, "bomb");
    sh.set_value(5, 0, "bomb");
    sh.set_value(5, 1, "bomb");
    sh.set_value(5, 2, "bomb");
    sh.set_col_span(3, 0, 2);
    sh.set_row_span(3, 0, 2);

    wb.push_sheet(sh);

    write_ods(&wb, "test_out/test_span.ods")?;

    let wi = read_ods_flags("test_out/test_span.ods", true)?;

    println!("{:?}", wi);


    let si = wi.sheet(0);

    assert_eq!(si.value(0, 0).as_str_or(""), "A");
    assert_eq!(si.col_span(0, 0), 2);

    let si = wi.sheet(1);

    assert_eq!(si.value(1, 0).as_str_or(""), "B");
    assert_eq!(si.row_span(1, 0), 2);

    Ok(())
}

