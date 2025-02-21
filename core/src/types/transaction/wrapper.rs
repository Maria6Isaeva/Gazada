/// Integration of Ferveo cryptographic primitives
/// to enable encrypted txs inside of normal txs.
/// *Not wasm compatible*
pub mod wrapper_tx {
    pub use ark_bls12_381::Bls12_381 as EllipticCurve;
    #[cfg(feature = "ferveo-tpke")]
    pub use ark_ec::{AffineCurve, PairingEngine};
    use borsh::{BorshDeserialize, BorshSchema, BorshSerialize};
    use serde::{Deserialize, Serialize};
    use sha2::{Digest, Sha256};
    use thiserror::Error;

    use crate::types::address::Address;
    use crate::types::key::*;
    use crate::types::storage::Epoch;
    use crate::types::token::Amount;

    /// Minimum fee amount in micro NAMs
    pub const MIN_FEE: u64 = 100;
    /// TODO: Determine a sane number for this
    const GAS_LIMIT_RESOLUTION: u64 = 1_000_000;

    /// Errors relating to decrypting a wrapper tx and its
    /// encrypted payload from a Tx type
    #[allow(missing_docs)]
    #[derive(Error, Debug)]
    pub enum WrapperTxErr {
        #[error(
            "The hash of the decrypted tx does not match the hash commitment"
        )]
        DecryptedHash,
        #[error("The decryption did not produce a valid Tx")]
        InvalidTx,
        #[error("The given Tx data did not contain a valid WrapperTx")]
        InvalidWrapperTx,
        #[error(
            "Attempted to sign WrapperTx with keypair whose public key \
             differs from that in the WrapperTx"
        )]
        InvalidKeyPair,
    }

    /// A fee is an amount of a specified token
    #[derive(
        Debug,
        Clone,
        PartialEq,
        BorshSerialize,
        BorshDeserialize,
        BorshSchema,
        Serialize,
        Deserialize,
        Eq,
    )]
    pub struct Fee {
        /// amount of the fee
        pub amount: Amount,
        /// address of the token
        pub token: Address,
    }

    /// Gas limits must be multiples of GAS_LIMIT_RESOLUTION
    /// This is done to minimize the amount of information leak from
    /// a wrapper tx. The larger the GAS_LIMIT_RESOLUTION, the
    /// less info leaked.
    ///
    /// This struct only stores the multiple of GAS_LIMIT_RESOLUTION,
    /// not the raw amount
    #[derive(
        Debug,
        Clone,
        PartialEq,
        Serialize,
        Deserialize,
        BorshSerialize,
        BorshDeserialize,
        BorshSchema,
        Eq,
    )]
    #[serde(from = "u64")]
    #[serde(into = "u64")]
    pub struct GasLimit {
        multiplier: u64,
    }

    impl GasLimit {
        /// We refund unused gas up to GAS_LIMIT_RESOLUTION
        pub fn refund_amount(&self, used_gas: u64) -> Amount {
            if used_gas < (u64::from(self) - GAS_LIMIT_RESOLUTION) {
                // we refund only up to GAS_LIMIT_RESOLUTION
                GAS_LIMIT_RESOLUTION
            } else if used_gas >= u64::from(self) {
                // Gas limit was under estimated, no refund
                0
            } else {
                // compute refund
                u64::from(self) - used_gas
            }
            .into()
        }
    }

    /// Round the input number up to the next highest multiple
    /// of GAS_LIMIT_RESOLUTION
    impl From<u64> for GasLimit {
        fn from(amount: u64) -> GasLimit {
            // we could use the ceiling function but this way avoids casts to
            // floats
            if GAS_LIMIT_RESOLUTION * (amount / GAS_LIMIT_RESOLUTION) < amount {
                GasLimit {
                    multiplier: (amount / GAS_LIMIT_RESOLUTION) + 1,
                }
            } else {
                GasLimit {
                    multiplier: (amount / GAS_LIMIT_RESOLUTION),
                }
            }
        }
    }

    /// Round the input number up to the next highest multiple
    /// of GAS_LIMIT_RESOLUTION
    impl From<Amount> for GasLimit {
        fn from(amount: Amount) -> GasLimit {
            GasLimit::from(u64::from(amount))
        }
    }

    /// Get back the gas limit as a raw number
    impl From<&GasLimit> for u64 {
        fn from(limit: &GasLimit) -> u64 {
            limit.multiplier * GAS_LIMIT_RESOLUTION
        }
    }

    /// Get back the gas limit as a raw number
    impl From<GasLimit> for u64 {
        fn from(limit: GasLimit) -> u64 {
            limit.multiplier * GAS_LIMIT_RESOLUTION
        }
    }

    /// Get back the gas limit as a raw number, viewed as an Amount
    impl From<GasLimit> for Amount {
        fn from(limit: GasLimit) -> Amount {
            Amount::from(limit.multiplier * GAS_LIMIT_RESOLUTION)
        }
    }

    /// A transaction with an encrypted payload as well
    /// as some non-encrypted metadata for inclusion
    /// and / or verification purposes
    #[derive(
        Debug,
        Clone,
        BorshSerialize,
        BorshDeserialize,
        BorshSchema,
        Serialize,
        Deserialize,
    )]
    pub struct WrapperTx {
        /// The fee to be payed for including the tx
        pub fee: Fee,
        /// Used to determine an implicit account of the fee payer
        pub pk: common::PublicKey,
        /// The epoch in which the tx is to be submitted. This determines
        /// which decryption key will be used
        pub epoch: Epoch,
        /// Max amount of gas that can be used when executing the inner tx
        pub gas_limit: GasLimit,
        #[cfg(not(feature = "mainnet"))]
        /// A PoW solution can be used to allow zero-fee testnet transactions
        pub pow_solution: Option<crate::ledger::testnet_pow::Solution>,
    }

    impl WrapperTx {
        /// Create a new wrapper tx from unencrypted tx, the personal keypair,
        /// and the metadata surrounding the inclusion of the tx. This method
        /// constructs the signature of relevant data and encrypts the
        /// transaction
        pub fn new(
            fee: Fee,
            keypair: &common::SecretKey,
            epoch: Epoch,
            gas_limit: GasLimit,
            #[cfg(not(feature = "mainnet"))] pow_solution: Option<
                crate::ledger::testnet_pow::Solution,
            >,
        ) -> WrapperTx {
            Self {
                fee,
                pk: keypair.ref_to(),
                epoch,
                gas_limit,
                #[cfg(not(feature = "mainnet"))]
                pow_solution,
            }
        }

        /// Get the address of the implicit account associated
        /// with the public key
        pub fn fee_payer(&self) -> Address {
            Address::from(&self.pk)
        }

        /// Produce a SHA-256 hash of this section
        pub fn hash<'a>(&self, hasher: &'a mut Sha256) -> &'a mut Sha256 {
            hasher.update(
                self.try_to_vec().expect("unable to serialize wrapper"),
            );
            hasher
        }
    }

    #[cfg(test)]
    mod test_gas_limits {
        use super::*;

        /// Test serializing and deserializing again gives back original object
        /// Test that serializing converts GasLimit to u64 correctly
        #[test]
        fn test_gas_limit_roundtrip() {
            let limit = GasLimit { multiplier: 1 };
            // Test serde roundtrip
            let js = serde_json::to_string(&limit).expect("Test failed");
            assert_eq!(js, format!("{}", GAS_LIMIT_RESOLUTION));
            let new_limit: GasLimit =
                serde_json::from_str(&js).expect("Test failed");
            assert_eq!(new_limit, limit);

            // Test borsh roundtrip
            let borsh = limit.try_to_vec().expect("Test failed");
            assert_eq!(
                limit,
                BorshDeserialize::deserialize(&mut borsh.as_ref())
                    .expect("Test failed")
            );
        }

        /// Test that when we deserialize a u64 that is not a multiple of
        /// GAS_LIMIT_RESOLUTION to a GasLimit, it rounds up to the next
        /// multiple
        #[test]
        fn test_deserialize_not_multiple_of_resolution() {
            let js = serde_json::to_string(&(GAS_LIMIT_RESOLUTION + 1))
                .expect("Test failed");
            let limit: GasLimit =
                serde_json::from_str(&js).expect("Test failed");
            assert_eq!(limit, GasLimit { multiplier: 2 });
        }

        /// Test that refund is calculated correctly
        #[test]
        fn test_gas_limit_refund() {
            let limit = GasLimit { multiplier: 1 };
            let refund = limit.refund_amount(GAS_LIMIT_RESOLUTION - 1);
            assert_eq!(refund, Amount::from(1u64));
        }

        /// Test that we don't refund more than GAS_LIMIT_RESOLUTION
        #[test]
        fn test_gas_limit_too_high_no_refund() {
            let limit = GasLimit { multiplier: 2 };
            let refund = limit.refund_amount(GAS_LIMIT_RESOLUTION - 1);
            assert_eq!(refund, Amount::from(GAS_LIMIT_RESOLUTION));
        }

        /// Test that if gas usage was underestimated, we issue no refund
        #[test]
        fn test_gas_limit_too_low_no_refund() {
            let limit = GasLimit { multiplier: 1 };
            let refund = limit.refund_amount(GAS_LIMIT_RESOLUTION + 1);
            assert_eq!(refund, Amount::from(0u64));
        }
    }

    #[cfg(test)]
    mod test_wrapper_tx {
        use super::*;
        use crate::proto::{Code, Data, Section, Signature, Tx, TxError};
        use crate::types::address::nam;
        use crate::types::transaction::{Hash, TxType};

        fn gen_keypair() -> common::SecretKey {
            use rand::prelude::ThreadRng;
            use rand::thread_rng;

            let mut rng: ThreadRng = thread_rng();
            ed25519::SigScheme::generate(&mut rng).try_to_sk().unwrap()
        }

        /// We test that when we feed in a Tx and then decrypt it again
        /// that we get what we started with.
        #[test]
        fn test_encryption_round_trip() {
            let keypair = gen_keypair();
            let mut wrapper =
                Tx::new(TxType::Wrapper(Box::new(WrapperTx::new(
                    Fee {
                        amount: 10.into(),
                        token: nam(),
                    },
                    &keypair,
                    Epoch(0),
                    0.into(),
                    #[cfg(not(feature = "mainnet"))]
                    None,
                ))));
            wrapper.set_code(Code::new("wasm code".as_bytes().to_owned()));
            wrapper
                .set_data(Data::new("transaction data".as_bytes().to_owned()));
            wrapper.add_section(Section::Signature(Signature::new(
                &wrapper.header_hash(),
                &keypair,
            )));
            let mut encrypted_tx = wrapper.clone();
            encrypted_tx.encrypt(&Default::default());
            assert!(encrypted_tx.validate_ciphertext());
            let privkey = <EllipticCurve as PairingEngine>::G2Affine::prime_subgroup_generator();
            encrypted_tx.decrypt(privkey).expect("Test failed");
            assert_eq!(wrapper.data(), encrypted_tx.data());
            assert_eq!(wrapper.code(), encrypted_tx.code());
        }

        /// We test that when we try to decrypt a tx and it
        /// does not match the commitment, an error is returned
        #[test]
        fn test_decryption_invalid_hash() {
            let keypair = gen_keypair();
            let mut wrapper =
                Tx::new(TxType::Wrapper(Box::new(WrapperTx::new(
                    Fee {
                        amount: 10.into(),
                        token: nam(),
                    },
                    &keypair,
                    Epoch(0),
                    0.into(),
                    #[cfg(not(feature = "mainnet"))]
                    None,
                ))));
            wrapper.set_code(Code::new("wasm code".as_bytes().to_owned()));
            wrapper
                .set_data(Data::new("transaction data".as_bytes().to_owned()));
            // give a incorrect commitment to the decrypted contents of the tx
            wrapper.set_code_sechash(Hash([0u8; 32]));
            wrapper.set_data_sechash(Hash([0u8; 32]));
            wrapper.add_section(Section::Signature(Signature::new(
                &wrapper.header_hash(),
                &keypair,
            )));
            wrapper.encrypt(&Default::default());
            assert!(wrapper.validate_ciphertext());
            let privkey = <EllipticCurve as PairingEngine>::G2Affine::prime_subgroup_generator();
            let err = wrapper.decrypt(privkey).expect_err("Test failed");
            assert_matches!(err, WrapperTxErr::DecryptedHash);
        }

        /// We check that even if the encrypted payload and hash of its
        /// contents are correctly changed, we detect fraudulent activity
        /// via the signature.
        #[test]
        fn test_malleability_attack_detection() {
            let keypair = gen_keypair();
            // the signed tx
            let mut tx = Tx::new(TxType::Wrapper(Box::new(WrapperTx::new(
                Fee {
                    amount: 10.into(),
                    token: nam(),
                },
                &keypair,
                Epoch(0),
                0.into(),
                #[cfg(not(feature = "mainnet"))]
                None,
            ))));

            tx.set_code(Code::new("wasm code".as_bytes().to_owned()));
            tx.set_data(Data::new("transaction data".as_bytes().to_owned()));
            tx.add_section(Section::Signature(Signature::new(
                &tx.header_hash(),
                &keypair,
            )));

            // we now try to alter the inner tx maliciously
            // malicious transaction
            // We replace the inner tx with a malicious one
            // We change the commitment appropriately
            let malicious = "Give me all the money".as_bytes().to_owned();
            tx.set_data(Data::new(malicious.clone()));
            tx.encrypt(&Default::default());

            // we check ciphertext validity still passes
            assert!(tx.validate_ciphertext());
            // we check that decryption still succeeds
            tx.decrypt(
                <EllipticCurve as PairingEngine>::G2Affine::prime_subgroup_generator(),
            )
                .expect("Test failed");
            assert_eq!(tx.data(), Some(malicious));

            // check that the signature is not valid
            tx.verify_signature(&keypair.ref_to(), &tx.header_hash())
                .expect_err("Test failed");
            // check that the try from method also fails
            let err = tx.validate_header().expect_err("Test failed");
            assert_matches!(err, TxError::SigError(_));
        }
    }
}

pub use wrapper_tx::*;
