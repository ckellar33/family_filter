use srp::client::SrpClient;
use srp::groups::G_3072;
use srp::rand::rand_bigint;
use sha2::Sha512;
use num_bigint::BigUint;
use anyhow::Result;

/// SRP Pair Setup for Apple TV
pub fn srp_pair_setup(pin: &str, salt: &[u8], pub_b_bytes: &[u8]) -> Result<(BigUint, Vec<u8>)> {
    let client = SrpClient::<Sha512>::new(&G_3072);
    let mut rng = rand::thread_rng();
    let a = rand_bigint::<_, 3072>(&mut rng);
    let (pub_a, sec_a) = client.compute_public_ephemeral(&a);
    let pub_b = BigUint::from_bytes_be(pub_b_bytes);
    let (key, proof_m1) = client.compute_key(
        &sec_a, &pub_a, &pub_b, salt, b"Pair-Setup", pin.as_bytes()
    )?;
    Ok((key, proof_m1))
}
