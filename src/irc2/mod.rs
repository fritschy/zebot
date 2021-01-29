mod parser;

pub fn parse_ng<'a>(i:&'a[u8]) {
    eprintln!("ParseNG: {:#?}", parser::parse(i));
}