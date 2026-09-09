//! Live EDDN tap for decode forensics: subscribe to the relay, filter
//! by schema, and print the raw JSON of anything the typed decoder
//! rejects — the instrument for "a new shape appeared in the wild".
//!
//!     cargo run -p ed-eddn --example tap -- outfitting [seconds]
//!
//! Prints every `modules` element (for outfitting) or whole message
//! (other schemas) that fails the current decode, plus a running count
//! of shapes seen.

use std::collections::BTreeMap;
use std::io::Read;

use zeromq::{Socket, SocketRecv, SubSocket};

fn inflate(frame: &[u8]) -> anyhow::Result<Vec<u8>> {
    if frame.first().is_some_and(|b| *b == b'{') {
        return Ok(frame.to_vec());
    }
    let mut out = Vec::with_capacity(frame.len() * 8);
    flate2::read::ZlibDecoder::new(frame).read_to_end(&mut out)?;
    Ok(out)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let want = args.first().map(String::as_str).unwrap_or("outfitting");
    let seconds: u64 = args.get(1).and_then(|v| v.parse().ok()).unwrap_or(120);
    let mut socket = SubSocket::new();
    socket.connect(ed_eddn::EDDN_RELAY).await?;
    socket.subscribe("").await?;
    eprintln!("tapping {} for schema *{want}* for {seconds}s", ed_eddn::EDDN_RELAY);

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(seconds);
    let mut matched = 0u64;
    let mut failures = 0u64;
    let mut shapes: BTreeMap<String, u64> = BTreeMap::new();
    while tokio::time::Instant::now() < deadline {
        let Ok(Ok(message)) = tokio::time::timeout_at(deadline, socket.recv()).await else {
            break;
        };
        let Some(frame) = message.get(0) else { continue };
        let Ok(json) = inflate(frame) else { continue };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&json) else { continue };
        let schema = value.get("$schemaRef").and_then(|v| v.as_str()).unwrap_or("");
        if !schema.contains(want) {
            continue;
        }
        matched += 1;
        let Some(msg) = value.get("message") else { continue };
        if want == "outfitting" {
            for module in msg.get("modules").and_then(|m| m.as_array()).map(|a| a.as_slice()).unwrap_or(&[]) {
                let shape = match module {
                    serde_json::Value::String(_) => "string".to_owned(),
                    serde_json::Value::Object(o) => {
                        let mut keys: Vec<&str> = o.keys().map(String::as_str).collect();
                        keys.sort_unstable();
                        format!("object{{{}}}", keys.join(","))
                    }
                    other => format!("{other:?}").chars().take(20).collect(),
                };
                *shapes.entry(shape).or_default() += 1;
                if serde_json::from_value::<ed_eddn::ModuleRef>(module.clone()).is_err() {
                    failures += 1;
                    if failures <= 5 {
                        println!("UNDECODABLE module element: {module}");
                    }
                }
            }
        } else if ed_eddn::decode(frame).is_err() {
            failures += 1;
            if failures <= 5 {
                println!("UNDECODABLE {schema}: {}", serde_json::to_string(msg)?);
            }
        }
    }
    eprintln!("{matched} {want} messages; {failures} undecodable; shapes: {shapes:?}");
    Ok(())
}
