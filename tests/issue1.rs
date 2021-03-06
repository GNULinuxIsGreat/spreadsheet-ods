use spreadsheet_ods::*;
use spreadsheet_ods::style::*;

#[test]
fn issue1() {
    let output_file = std::path::Path::new("test_out/test.ods");

    let mut wb = WorkBook::new();

    // let mut style = Style::new_cell_style("Square style", "");
    // style.col_mut().set_col_width(Length::Cm(10.0));
    // style.row_mut().set_row_height(Length::Cm(10.0));
    // wb.add_style(style);

    let mut sheet = spreadsheet_ods::Sheet::new_with_name("test");
    sheet.set_row_height(&mut wb, 1, cm!(10));
    sheet.set_col_width(&mut wb, 1, cm!(10));
    sheet.set_value(1, 1, ());
    //sheet.cell_mut(1, 1).set_style("Square style");
    wb.push_sheet(sheet);

    if let Err(x) = spreadsheet_ods::write_ods(&wb, output_file) {
        println!("Error: {}", x)
    }
}
