//! Explicit device delegation to one ephemeral transport and application session.
use super::*;
use crate::{
    events::SessionId,
    identity::{DeviceId, DeviceIdentity},
};
const MAGIC: &[u8; 5] = b"OCDO\x01";
const DOMAIN: &[u8] = b"our-clipboard/relay-device-offer/v1\0";
const HEADER: usize = 90;
const SIGNATURE: usize = 64;
/// Signature validity is not trust: callers must independently confirm the device ID.
/// Public bytes only; no enrollment, membership authority or saved peer trust is implied.
pub struct SignedRelayOffer {
    wire: Vec<u8>,
    offer: PublicOffer,
    key: [u8; 32],
    session: SessionId,
}
impl SignedRelayOffer {
    pub const MAX_WIRE_BYTES: usize = HEADER + 32_777 + SIGNATURE;
    pub fn new(
        identity: &DeviceIdentity,
        session: SessionId,
        room: Room,
        side: Side,
        offer: &PublicOffer,
    ) -> R<Self> {
        let payload = offer.to_wire();
        let mut bytes = MAGIC.to_vec();
        bytes.extend(identity.public_key());
        bytes.extend(session.0);
        bytes.extend(room.0);
        bytes.push(side.byte());
        bytes.extend((payload.len() as u32).to_be_bytes());
        bytes.extend(payload);
        let mut message = DOMAIN.to_vec();
        message.extend(&bytes);
        bytes.extend(identity.sign(&message));
        Self::from_wire(&bytes, room, side)
    }
    pub fn from_wire(bytes: &[u8], expected_room: Room, expected_side: Side) -> R<Self> {
        if !(HEADER + 10 + SIGNATURE..=Self::MAX_WIRE_BYTES).contains(&bytes.len())
            || &bytes[..5] != MAGIC
        {
            return Err(RelayError::Configuration);
        }
        let payload_len = u32::from_be_bytes(
            bytes[86..90]
                .try_into()
                .map_err(|_| RelayError::Configuration)?,
        ) as usize;
        if payload_len != bytes.len() - HEADER - SIGNATURE {
            return Err(RelayError::Configuration);
        }
        if bytes[53..85] != expected_room.0
            || bytes[85] != expected_side.byte()
            || expected_room.0 == [0; 32]
            || bytes[37..53] == [0; 16]
        {
            return Err(RelayError::PeerAuthentication);
        }
        let key = bytes[5..37]
            .try_into()
            .map_err(|_| RelayError::Configuration)?;
        let end = bytes.len() - SIGNATURE;
        let mut message = DOMAIN.to_vec();
        message.extend(&bytes[..end]);
        DeviceIdentity::verify(
            &key,
            &message,
            bytes[end..]
                .try_into()
                .map_err(|_| RelayError::Configuration)?,
        )
        .map_err(|_| RelayError::PeerAuthentication)?;
        let offer = PublicOffer::from_wire(&bytes[HEADER..end])?;
        Ok(Self {
            wire: bytes.to_vec(),
            offer,
            key,
            session: SessionId(
                bytes[37..53]
                    .try_into()
                    .map_err(|_| RelayError::Configuration)?,
            ),
        })
    }
    pub fn to_wire(&self) -> &[u8] {
        &self.wire
    }
    pub fn device_id(&self) -> DeviceId {
        DeviceId::from_public_key(&self.key)
    }
    pub fn public_key(&self) -> [u8; 32] {
        self.key
    }
    pub fn session(&self) -> SessionId {
        self.session
    }
    pub fn into_transport_offer(self) -> PublicOffer {
        self.offer
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_signed_field_is_bound_and_legacy_or_wrong_context_never_downgrades() {
        let signer = DeviceIdentity::generate().unwrap();
        let tls = RelayIdentity::generate().unwrap().public_offer();
        let session = SessionId::random().unwrap();
        let room = Room([1; 32]);
        let signed = SignedRelayOffer::new(&signer, session, room, Side::A, &tls).unwrap();
        assert_eq!(signed.device_id(), signer.id());
        assert_eq!(signed.public_key(), signer.public_key());
        assert_eq!(signed.session(), session);
        assert!(SignedRelayOffer::from_wire(signed.to_wire(), Room([2; 32]), Side::A).is_err());
        assert!(SignedRelayOffer::from_wire(signed.to_wire(), room, Side::B).is_err());
        assert!(SignedRelayOffer::from_wire(&tls.to_wire(), room, Side::A).is_err());
        for i in 0..signed.to_wire().len() {
            let mut bytes = signed.to_wire().to_vec();
            bytes[i] ^= 1;
            assert!(
                SignedRelayOffer::from_wire(&bytes, room, Side::A).is_err(),
                "mutation at {i}"
            );
        }
        for n in 0..signed.to_wire().len() {
            assert!(SignedRelayOffer::from_wire(&signed.to_wire()[..n], room, Side::A).is_err());
        }
        let mut bytes = signed.to_wire().to_vec();
        bytes.push(0);
        assert!(SignedRelayOffer::from_wire(&bytes, room, Side::A).is_err());
        assert_eq!(
            signed.into_transport_offer().fingerprint(),
            tls.fingerprint()
        );
    }
    #[test]
    fn one_device_keeps_its_id_across_fresh_transports_and_sessions() {
        let signer = DeviceIdentity::generate().unwrap();
        let a = SignedRelayOffer::new(
            &signer,
            SessionId::random().unwrap(),
            Room([1; 32]),
            Side::A,
            &RelayIdentity::generate().unwrap().public_offer(),
        )
        .unwrap();
        let b = SignedRelayOffer::new(
            &signer,
            SessionId::random().unwrap(),
            Room([1; 32]),
            Side::A,
            &RelayIdentity::generate().unwrap().public_offer(),
        )
        .unwrap();
        assert_eq!(a.device_id(), b.device_id());
        assert_ne!(a.session(), b.session());
        assert_ne!(a.to_wire(), b.to_wire());
    }
}
