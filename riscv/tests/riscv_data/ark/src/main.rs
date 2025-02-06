#![no_std]
#![no_main]

// use std::fs::File;
// use std::io::{Read, Write};
// use std::io::{BufReader, BufWriter};
use ark_ec::bn::Bn;
use ark_serialize::{CanonicalSerialize, CanonicalDeserialize, SerializationError};
use ark_std::vec;

// For randomness (during paramgen and proof generation)
// use ark_std::rand::{Rng, RngCore, SeedableRng};
use rand_chacha::ChaCha20Rng;
use rand_core::{RngCore, SeedableRng};
use ark_ff::vec::Vec;
use powdr_riscv_runtime::{print, io};
// For benchmarking

// Bring in some tools for using pairing-friendly curves
// We're going to use the BLS12-377 pairing-friendly elliptic curve.
// use ark_bls12_377::{Bls12_377, Fr};
use ark_bn254::{Bn254, Config, Fr};
use ark_ff::{BigInt, Field, Fp};
use ark_std::test_rng;
use ark_groth16::{ProvingKey, PreparedVerifyingKey};
use ark_crypto_primitives::snark::{CircuitSpecificSetupSNARK, SNARK};
// We'll use these interfaces to construct our circuit.
use ark_relations::{
    lc, ns,
    r1cs::{ConstraintSynthesizer, ConstraintSystemRef, SynthesisError, Variable},
};

const PROOF_CHANNEL: u32 = 666;
const PVK_CHANNEL: u32 = 667;

#[no_mangle]
fn main() {
    use ark_groth16::Groth16;

    let proof_bytes: Vec<u8> = io::read(PROOF_CHANNEL);
    let pvk_bytes: Vec<u8> = io::read(PVK_CHANNEL);
    let big_int_value = BigInt::<4>::new([
        1875955372304588914,
        12194129877466962247,
        15183177813418508560,
        2843644298302705624,
    ]);

    let image: Fp<ark_ff::MontBackend<ark_bn254::FrConfig, 4>, 4> = Fr::from(big_int_value);
    
    let deserialized_pvk: PreparedVerifyingKey<Bn254> = {
        PreparedVerifyingKey::deserialize_uncompressed(&mut &pvk_bytes[..]).unwrap()
    };
    let deserialized_proof: ark_groth16::Proof<Bn<Config>> = {
        ark_groth16::Proof::deserialize_uncompressed(&mut &proof_bytes[..]).unwrap()
    };

    Groth16::<Bn254>::verify_with_processed_vk(&deserialized_pvk, &[image], &deserialized_proof).unwrap();
}
