use oxc_allocator::Allocator;
use oxc_parser::Parser;
use oxc_span::SourceType;

fn main() {
    let allocator = Allocator::default();
    let source_text = "const a = 1;";
    let source_type = SourceType::default();
    let ret = Parser::new(&allocator, source_text, source_type).parse();
    println!("{:?}", ret.program.body);
}
