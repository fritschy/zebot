mod parser;

pub fn parse_ng<'a>(i:&'a[u8]) {
    parser::parse(i);
}