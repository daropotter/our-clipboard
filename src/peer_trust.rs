//! Explicit local approval commands; no network/clipboard/group enrollment.
use crate::{
    identity::{self, DeviceId},
    transport::outbound::{Room, Side, SignedRelayOffer},
};
use std::{
    collections::HashSet,
    fs::File,
    io::{BufRead, Read},
    path::Path,
};
const HELP: &str = "Usage: our-clipboard peer trust --name NAME --offer FILE --room HEX --side a|b [--profile PROFILE]\n       our-clipboard peer show --name NAME [--profile PROFILE]\n\nTrust requires an existing system-store identity, a valid signed peer offer and independent full device-fingerprint confirmation. Side is the offer issuer's role. Saves a new immutable local peer approval; existing names are never replaced. No network connection, clipboard access, group membership or invitation authority.";
struct Options {
    trust: bool,
    profile: String,
    name: String,
    offer: Option<String>,
    room: Option<Room>,
    side: Option<Side>,
}
fn options(args: &[String]) -> Result<Options, &'static str> {
    let trust = match args.first().map(String::as_str) {
        Some("trust") => true,
        Some("show") => false,
        _ => return Err("Use peer trust or peer show; see peer --help."),
    };
    let mut o = Options {
        trust,
        profile: "default".into(),
        name: String::new(),
        offer: None,
        room: None,
        side: None,
    };
    let mut seen = HashSet::new();
    for pair in args[1..].chunks(2) {
        if pair.len() != 2 || !seen.insert(pair[0].as_str()) {
            return Err("Missing or repeated peer option.");
        }
        let v = &pair[1];
        match pair[0].as_str() {
            "--profile" => o.profile = v.clone(),
            "--name" => o.name = v.clone(),
            "--offer" => o.offer = Some(v.clone()),
            "--room" => {
                o.room = Some(Room::parse(v).map_err(|_| "Invalid offer room routing ID.")?)
            }
            "--side" => {
                o.side = Some(match v.as_str() {
                    "a" => Side::A,
                    "b" => Side::B,
                    _ => return Err("Offer side must be a or b."),
                })
            }
            _ => return Err("Unknown peer option."),
        }
    }
    identity::validate_peer_name(&o.name).map_err(|e| e.message())?;
    if trust && (o.offer.is_none() || o.room.is_none() || o.side.is_none()) {
        return Err("peer trust requires --offer, --room and --side.");
    }
    if !trust && (o.offer.is_some() || o.room.is_some() || o.side.is_some()) {
        return Err("peer show accepts only --name and --profile.");
    }
    Ok(o)
}
fn read_offer(path: &Path, room: Room, side: Side) -> Result<SignedRelayOffer, &'static str> {
    if !std::fs::symlink_metadata(path)
        .map_err(|_| "Peer offer unavailable.")?
        .file_type()
        .is_file()
    {
        return Err("Peer offer must be a regular file, not a symlink.");
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|_| "Peer offer unavailable.")?
        .take((SignedRelayOffer::MAX_WIRE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "Could not read peer offer.")?;
    SignedRelayOffer::from_wire(&bytes, room, side).map_err(
        |_| "Signed peer offer has invalid signature, room, side or encoding; nothing was saved.",
    )
}
fn confirm(input: &mut impl BufRead, expected: &str) -> Result<(), &'static str> {
    let limit = (expected.len() + 3) as u64;
    let mut text = String::new();
    input
        .take(limit)
        .read_line(&mut text)
        .map_err(|_| "Could not read peer approval.")?;
    if text.len() as u64 >= limit || text.trim() != expected {
        return Err("Peer fingerprint approval failed; no trust was saved.");
    }
    Ok(())
}
pub fn run(args: &[String]) -> Result<String, &'static str> {
    if matches!(args,[arg] if arg=="--help" || arg=="-h") {
        return Ok(HELP.into());
    }
    let o = options(args)?;
    let owner = identity::open_existing(&o.profile).map_err(|e| e.message())?;
    let peer = if o.trust {
        let peer = read_offer(
            Path::new(o.offer.as_ref().unwrap()),
            o.room.unwrap(),
            o.side.unwrap(),
        )?;
        let fingerprint = peer.device_id().fingerprint();
        if peer.public_key() == owner.public_key() {
            return Err("A peer approval must identify another device.");
        }
        eprintln!("Local device: {}", owner.id().fingerprint());
        eprintln!("Approve PUBLIC peer device: {fingerprint}");
        eprintln!(
            "Compare with the other computer's identity show output through an independent trusted channel, then enter that full fingerprint. This explicitly saves local trust for the selected name:"
        );
        confirm(&mut std::io::stdin().lock(), &fingerprint)?;
        identity::remember_peer(&o.profile, &o.name, &owner, peer.public_key())
            .map_err(|e| e.message())?;
        peer.public_key()
    } else {
        identity::load_peer(&o.profile, &o.name, &owner).map_err(|e| e.message())?
    };
    Ok(format!(
        "Local peer approval: {}\nPeer device: {}\nScope: this local identity and peer name; no group authority.",
        o.name,
        DeviceId::from_public_key(&peer).fingerprint()
    ))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_options_and_full_fingerprint_are_required() {
        let show = ["show", "--name", "mac"].map(str::to_string);
        assert!(options(&show).is_ok());
        for args in [
            vec!["trust", "--name", "mac"],
            vec!["show", "--name", "../mac"],
            vec!["show", "--name", "mac", "--offer", "x"],
            vec!["show", "--name", "a", "--name", "b"],
        ] {
            assert!(options(&args.into_iter().map(str::to_string).collect::<Vec<_>>()).is_err());
        }
        let fp = identity::DeviceIdentity::generate()
            .unwrap()
            .id()
            .fingerprint();
        assert!(confirm(&mut std::io::Cursor::new(format!("{fp}\r\n")), &fp).is_ok());
        for text in [
            "yes".to_string(),
            format!("{fp}x"),
            fp[..fp.len() - 1].to_string(),
            format!("{fp}   "),
        ] {
            assert!(confirm(&mut std::io::Cursor::new(text), &fp).is_err());
        }
    }
}
