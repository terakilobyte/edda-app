use std::io::Cursor;

fn main() -> anyhow::Result<()> {
    let source = br#"[
{"id64":10477373803,"name":"Sol","coords":{"x":0.0,"y":0.0,"z":0.0},"bodies":[{"type":"Star","subType":"G (White-Yellow) Star","mainStar":true},{"type":"Star","subType":"K (Yellow-Orange) Star","mainStar":false,"distanceToArrival":300.0}]},
{"id64":2,"name":"Alpha","coords":{"x":60.0,"y":0.0,"z":0.0},"bodies":[{"type":"Star","subType":"K (Yellow-Orange) Star","mainStar":true}]},
{"id64":3,"name":"beta","coords":{"x":-60.0,"y":0.0,"z":0.0},"bodies":[{"type":"Star","subType":"Neutron Star","mainStar":true}]}
]"#;
    let directory = tempfile::tempdir()?;
    ed_galaxy::import::import_reader(Box::new(Cursor::new(source)), directory.path(), &mut |_| {})?;
    for name in ["stars.bin", "cells.bin", "names.bin", "byname.bin"] {
        println!("[{name}]");
        for chunk in std::fs::read(directory.path().join(name))?.chunks(16) {
            for byte in chunk {
                print!("{byte:02x}");
            }
            println!();
        }
    }
    Ok(())
}
