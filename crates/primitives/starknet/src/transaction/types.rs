use alloc::collections::BTreeMap;
use alloc::string::String;

use blockifier::execution::entry_point::CallInfo;
use blockifier::execution::errors::EntryPointExecutionError;
use blockifier::state::errors::StateError;
use blockifier::transaction::errors::TransactionExecutionError;
use blockifier::transaction::transaction_types::TransactionType;
use frame_support::BoundedVec;
use sp_core::{ConstU32, U256};
use starknet_api::transaction::Fee;
use starknet_api::StarknetApiError;
#[cfg(feature = "std")]
use starknet_core::types::{
    DeclareTransaction as RPCDeclareTransaction, DeclareTransactionReceipt as RPCDeclareTransactionReceipt,
    DeclareTransactionV1 as RPCDeclareTransactionV1, DeclareTransactionV2 as RPCDeclareTransactionV2,
    DeployAccountTransaction as RPCDeployAccountTransaction,
    DeployAccountTransactionReceipt as RPCDeployAccountTransactionReceipt, Event as RPCEvent, FieldElement,
    InvokeTransaction as RPCInvokeTransaction, InvokeTransactionReceipt as RPCInvokeTransactionReceipt,
    InvokeTransactionV0 as RPCInvokeTransactionV0, InvokeTransactionV1 as RPCInvokeTransactionV1,
    L1HandlerTransaction as RPCL1HandlerTransaction, L1HandlerTransactionReceipt as RPCL1HandlerTransactionReceipt,
    MaybePendingTransactionReceipt as RPCMaybePendingTransactionReceipt, Transaction as RPCTransaction,
    TransactionReceipt as RPCTransactionReceipt, TransactionStatus as RPCTransactionStatus,
};
use thiserror_no_std::Error;

use crate::crypto::commitment::{
    calculate_declare_tx_hash, calculate_deploy_account_tx_hash, calculate_invoke_tx_hash,
};
use crate::execution::call_entrypoint_wrapper::MaxCalldataSize;
use crate::execution::entrypoint_wrapper::EntryPointTypeWrapper;
use crate::execution::types::{
    CallEntryPointWrapper, ContractAddressWrapper, ContractClassWrapper, Felt252Wrapper, Felt252WrapperError,
};

/// Max size of arrays.
/// TODO: add real value (#250)
pub type MaxArraySize = ConstU32<10000>;

/// Wrapper type for transaction execution result.
pub type TransactionExecutionResultWrapper<T> = Result<T, TransactionExecutionErrorWrapper>;

/// Wrapper type for transaction execution error.
#[derive(Debug, Error)]
pub enum TransactionExecutionErrorWrapper {
    /// Transaction execution error.
    #[error(transparent)]
    TransactionExecution(#[from] TransactionExecutionError),
    /// Starknet API error.
    #[error(transparent)]
    StarknetApi(#[from] StarknetApiError),
    /// Block context serialization error.
    #[error("Block context serialization error")]
    BlockContextSerializationError,
    /// State error.
    #[error(transparent)]
    StateError(#[from] StateError),
    /// Fee computation error,
    #[error("Fee computation error")]
    FeeComputationError,
    /// Fee transfer error,
    #[error("Fee transfer error. Max fee is {}, Actual fee is {}", max_fee.0, actual_fee.0)]
    FeeTransferError {
        /// Max fee specified by the set.
        max_fee: Fee,
        /// Actual fee.
        actual_fee: Fee,
    },
    /// Cairo resources are not contained in the fee costs.
    #[error("Cairo resources are not contained in the fee costs")]
    CairoResourcesNotContainedInFeeCosts,
    /// Failed to compute the L1 gas usage.
    #[error("Failed to compute the L1 gas usage")]
    FailedToComputeL1GasUsage,
    /// Entrypoint execution error
    #[error(transparent)]
    EntrypointExecution(#[from] EntryPointExecutionError),
    /// Unexpected holes.
    #[error("Unexpected holes: {0}")]
    UnexpectedHoles(String),
}

impl From<TransactionValidationErrorWrapper> for TransactionExecutionErrorWrapper {
    fn from(error: TransactionValidationErrorWrapper) -> Self {
        match error {
            TransactionValidationErrorWrapper::TransactionValidationError(e) => Self::TransactionExecution(e),
            TransactionValidationErrorWrapper::CalldataError(e) => Self::StarknetApi(e),
        }
    }
}

/// Wrapper type for transaction validation result.
pub type TransactionValidationResultWrapper<T> = Result<T, TransactionValidationErrorWrapper>;

/// Wrapper type for transaction validation error.
#[derive(Debug, Error)]
pub enum TransactionValidationErrorWrapper {
    /// Transaction execution error
    #[error(transparent)]
    TransactionValidationError(#[from] TransactionExecutionError),
    /// Calldata error
    #[error(transparent)]
    CalldataError(#[from] StarknetApiError),
}

impl From<EntryPointExecutionError> for TransactionValidationErrorWrapper {
    fn from(error: EntryPointExecutionError) -> Self {
        Self::TransactionValidationError(TransactionExecutionError::from(error))
    }
}

/// Different tx types.
/// See `https://docs.starknet.io/documentation/architecture_and_concepts/Blocks/transactions/` for more details.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    scale_codec::Encode,
    scale_codec::Decode,
    scale_info::TypeInfo,
    scale_codec::MaxEncodedLen,
)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub enum TxType {
    /// Regular invoke transaction.
    Invoke,
    /// Declare transaction.
    Declare,
    /// Deploy account transaction.
    DeployAccount,
    /// Message sent from ethereum.
    L1Handler,
}
impl From<TransactionType> for TxType {
    fn from(value: TransactionType) -> Self {
        match value {
            TransactionType::Declare => Self::Declare,
            TransactionType::DeployAccount => Self::DeployAccount,
            TransactionType::InvokeFunction => Self::Invoke,
            TransactionType::L1Handler => Self::L1Handler,
        }
    }
}
impl From<TxType> for TransactionType {
    fn from(value: TxType) -> Self {
        match value {
            TxType::Declare => Self::Declare,
            TxType::DeployAccount => Self::DeployAccount,
            TxType::Invoke => Self::InvokeFunction,
            TxType::L1Handler => Self::L1Handler,
        }
    }
}

/// Declare transaction.
#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    scale_codec::Encode,
    scale_codec::Decode,
    scale_info::TypeInfo,
    scale_codec::MaxEncodedLen,
)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct DeclareTransaction {
    /// Transaction version.
    pub version: u8,
    /// Transaction sender address.
    pub sender_address: ContractAddressWrapper,
    /// Class hash to declare.
    pub compiled_class_hash: Felt252Wrapper,
    /// Contract to declare.
    pub contract_class: ContractClassWrapper,
    /// Account contract nonce.
    pub nonce: Felt252Wrapper,
    /// Transaction signature.
    pub signature: BoundedVec<Felt252Wrapper, MaxArraySize>,
    /// Max fee.
    pub max_fee: Felt252Wrapper,
}

impl DeclareTransaction {
    /// converts the transaction to a [Transaction] object
    pub fn from_declare(self, chain_id: &str) -> Transaction {
        Transaction {
            tx_type: TxType::Declare,
            version: self.version,
            hash: calculate_declare_tx_hash(self.clone(), chain_id),
            signature: self.signature,
            sender_address: self.sender_address,
            nonce: self.nonce,
            call_entrypoint: CallEntryPointWrapper::new(
                Some(self.compiled_class_hash),
                EntryPointTypeWrapper::External,
                None,
                BoundedVec::default(),
                self.sender_address,
                self.sender_address,
            ),
            contract_class: Some(self.contract_class),
            contract_address_salt: None,
            max_fee: self.max_fee,
        }
    }
}

/// Deploy account transaction.
#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    scale_codec::Encode,
    scale_codec::Decode,
    scale_info::TypeInfo,
    scale_codec::MaxEncodedLen,
)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct DeployAccountTransaction {
    /// Transaction version.
    pub version: u8,
    /// Transaction sender address.
    pub sender_address: ContractAddressWrapper,
    /// Transaction calldata.
    pub calldata: BoundedVec<Felt252Wrapper, MaxCalldataSize>,
    /// Account contract nonce.
    pub nonce: Felt252Wrapper,
    /// Transaction salt.
    pub salt: U256,
    /// Transaction signature.
    pub signature: BoundedVec<Felt252Wrapper, MaxArraySize>,
    /// Account class hash.
    pub account_class_hash: Felt252Wrapper,
    /// Max fee.
    pub max_fee: Felt252Wrapper,
}

impl DeployAccountTransaction {
    /// converts the transaction to a [Transaction] object
    pub fn from_deploy(self, chain_id: &str) -> Transaction {
        Transaction {
            tx_type: TxType::DeployAccount,
            version: self.version,
            hash: calculate_deploy_account_tx_hash(self.clone(), chain_id),
            signature: self.signature,
            sender_address: self.sender_address,
            nonce: self.nonce,
            call_entrypoint: CallEntryPointWrapper::new(
                Some(self.account_class_hash),
                EntryPointTypeWrapper::External,
                None,
                self.calldata,
                self.sender_address,
                self.sender_address,
            ),
            contract_class: None,
            contract_address_salt: Some(self.salt),
            max_fee: self.max_fee,
        }
    }
}

/// Error of conversion between [DeclareTransaction], [InvokeTransaction],
/// [DeployAccountTransaction] and [Transaction].
#[derive(Debug, Error)]
pub enum TransactionConversionError {
    /// Class hash is missing from the object of type [Transaction]
    #[error("Class hash is missing from the object of type [Transaction]")]
    MissingClassHash,
    /// Class is missing from the object of type [Transaction]
    #[error("Class is missing from the object of type [Transaction]")]
    MissingClass,
}
impl TryFrom<Transaction> for DeclareTransaction {
    type Error = TransactionConversionError;
    fn try_from(value: Transaction) -> Result<Self, Self::Error> {
        Ok(Self {
            version: value.version,
            signature: value.signature,
            sender_address: value.sender_address,
            nonce: value.nonce,
            contract_class: value.contract_class.ok_or(TransactionConversionError::MissingClass)?,
            compiled_class_hash: value
                .call_entrypoint
                .class_hash
                .ok_or(TransactionConversionError::MissingClassHash)?,
            max_fee: value.max_fee,
        })
    }
}

/// Invoke transaction.
#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    scale_codec::Encode,
    scale_codec::Decode,
    scale_info::TypeInfo,
    scale_codec::MaxEncodedLen,
)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct InvokeTransaction {
    /// Transaction version.
    pub version: u8,
    /// Transaction sender address.
    pub sender_address: ContractAddressWrapper,
    /// Transaction calldata.
    pub calldata: BoundedVec<Felt252Wrapper, MaxCalldataSize>,
    /// Account contract nonce.
    pub nonce: Felt252Wrapper,
    /// Transaction signature.
    pub signature: BoundedVec<Felt252Wrapper, MaxArraySize>,
    /// Max fee.
    pub max_fee: Felt252Wrapper,
}

impl From<Transaction> for InvokeTransaction {
    fn from(value: Transaction) -> Self {
        Self {
            version: value.version,
            signature: value.signature,
            sender_address: value.sender_address,
            nonce: value.nonce,
            calldata: value.call_entrypoint.calldata,
            max_fee: value.max_fee,
        }
    }
}

impl InvokeTransaction {
    /// converts the transaction to a [Transaction] object
    pub fn from_invoke(self, chain_id: &str) -> Transaction {
        Transaction {
            tx_type: TxType::Invoke,
            version: self.version,
            hash: calculate_invoke_tx_hash(self.clone(), chain_id),
            signature: self.signature,
            sender_address: self.sender_address,
            nonce: self.nonce,
            call_entrypoint: CallEntryPointWrapper::new(
                None,
                EntryPointTypeWrapper::External,
                None,
                self.calldata,
                self.sender_address,
                self.sender_address,
            ),
            contract_class: None,
            contract_address_salt: None,
            max_fee: self.max_fee,
        }
    }
}

/// Representation of a Starknet transaction.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    scale_codec::Encode,
    scale_codec::Decode,
    scale_info::TypeInfo,
    scale_codec::MaxEncodedLen,
)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct Transaction {
    /// The type of the transaction.
    pub tx_type: TxType,
    /// The version of the transaction.
    pub version: u8,
    /// Transaction hash.
    pub hash: Felt252Wrapper,
    /// Signature.
    pub signature: BoundedVec<Felt252Wrapper, MaxArraySize>,
    /// Sender Address
    pub sender_address: ContractAddressWrapper,
    /// Nonce
    pub nonce: Felt252Wrapper,
    /// Call entrypoint
    pub call_entrypoint: CallEntryPointWrapper,
    /// Contract Class
    pub contract_class: Option<ContractClassWrapper>,
    /// Contract Address Salt
    pub contract_address_salt: Option<U256>,
    /// Max fee.
    pub max_fee: Felt252Wrapper,
}

impl TryFrom<Transaction> for DeployAccountTransaction {
    type Error = TransactionConversionError;
    fn try_from(value: Transaction) -> Result<Self, Self::Error> {
        Ok(Self {
            version: value.version,
            signature: value.signature,
            sender_address: value.sender_address,
            nonce: value.nonce,
            calldata: value.call_entrypoint.calldata,
            salt: value.contract_address_salt.unwrap_or_default(),
            account_class_hash: value.call_entrypoint.class_hash.ok_or(TransactionConversionError::MissingClassHash)?,
            max_fee: value.max_fee,
        })
    }
}

/// Error of conversion between the Madara Primitive Transaction and the RPC Transaction
#[cfg(feature = "std")]
#[derive(Debug, Error)]
pub enum RPCTransactionConversionError {
    /// The u8 stored version doesn't match any of the existing version at the RPC level
    #[error("Unknown version")]
    UnknownVersion,
    /// Missing information
    #[error("Missing information")]
    MissingInformation,
    /// Conversion from byte array has failed.
    #[error("Conversion from byte array has failed")]
    FromArrayError,
    /// Provided byte array has incorrect lengths.
    #[error("Provided byte array has incorrect lengths")]
    InvalidLength,
    /// Invalid character in hex string.
    #[error("Invalid character in hex string")]
    InvalidCharacter,
    /// Value is too large for FieldElement (felt252).
    #[error("Value is too large for FieldElement (felt252)")]
    OutOfRange,
    /// Value is too large to fit into target type.
    #[error("Value is too large to fit into target type")]
    ValueTooLarge,
}

#[cfg(feature = "std")]
impl From<Felt252WrapperError> for RPCTransactionConversionError {
    fn from(value: Felt252WrapperError) -> Self {
        match value {
            Felt252WrapperError::FromArrayError => Self::FromArrayError,
            Felt252WrapperError::InvalidLength => Self::InvalidLength,
            Felt252WrapperError::InvalidCharacter => Self::InvalidCharacter,
            Felt252WrapperError::OutOfRange => Self::OutOfRange,
            Felt252WrapperError::ValueTooLarge => Self::ValueTooLarge,
        }
    }
}

#[cfg(feature = "std")]
impl TryFrom<Transaction> for RPCTransaction {
    type Error = RPCTransactionConversionError;
    fn try_from(value: Transaction) -> Result<Self, Self::Error> {
        let transaction_hash = value.hash.0;
        let max_fee = value.max_fee.0;
        let signature = value.signature.iter().map(|&f| f.0).collect();
        let nonce = value.nonce.0;
        let sender_address = value.sender_address.0;
        let class_hash = value.call_entrypoint.class_hash.ok_or(RPCTransactionConversionError::MissingInformation);
        let contract_address = value.call_entrypoint.storage_address.0;
        let entry_point_selector =
            value.call_entrypoint.entrypoint_selector.ok_or(RPCTransactionConversionError::MissingInformation);
        let calldata = value.call_entrypoint.calldata.iter().map(|&f| f.0).collect();

        match value.tx_type {
            TxType::Declare => {
                let class_hash = class_hash?.0;
                match value.version {
                    1 => Ok(RPCTransaction::Declare(RPCDeclareTransaction::V1(RPCDeclareTransactionV1 {
                        transaction_hash,
                        max_fee,
                        signature,
                        nonce,
                        class_hash,
                        sender_address,
                    }))),
                    2 => Ok(RPCTransaction::Declare(RPCDeclareTransaction::V2(RPCDeclareTransactionV2 {
                        transaction_hash,
                        max_fee,
                        signature,
                        nonce,
                        class_hash,
                        sender_address,
                        compiled_class_hash: class_hash,
                    }))),
                    _ => Err(RPCTransactionConversionError::UnknownVersion),
                }
            }
            TxType::Invoke => match value.version {
                0 => Ok(RPCTransaction::Invoke(RPCInvokeTransaction::V0(RPCInvokeTransactionV0 {
                    transaction_hash,
                    max_fee,
                    signature,
                    nonce,
                    contract_address,
                    entry_point_selector: entry_point_selector?.0,
                    calldata,
                }))),
                1 => Ok(RPCTransaction::Invoke(RPCInvokeTransaction::V1(RPCInvokeTransactionV1 {
                    transaction_hash,
                    max_fee,
                    signature,
                    nonce,
                    sender_address,
                    calldata,
                }))),
                _ => Err(RPCTransactionConversionError::UnknownVersion),
            },
            TxType::DeployAccount => Ok(RPCTransaction::DeployAccount(RPCDeployAccountTransaction {
                transaction_hash,
                max_fee,
                signature,
                nonce,
                contract_address_salt: Felt252Wrapper::try_from(
                    value.contract_address_salt.ok_or(RPCTransactionConversionError::MissingInformation)?,
                )?
                .0,
                constructor_calldata: calldata,
                class_hash: class_hash?.0,
            })),
            TxType::L1Handler => {
                let nonce = TryInto::try_into(value.nonce).unwrap(); // this panics in case of overflow
                Ok(RPCTransaction::L1Handler(RPCL1HandlerTransaction {
                    transaction_hash,
                    version: value.version.into(),
                    nonce,
                    contract_address,
                    entry_point_selector: entry_point_selector?.0,
                    calldata,
                }))
            }
        }
    }
}

/// Representation of a Starknet transaction receipt.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    scale_codec::Encode,
    scale_codec::Decode,
    scale_info::TypeInfo,
    scale_codec::MaxEncodedLen,
)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct TransactionReceiptWrapper {
    /// Transaction hash.
    pub transaction_hash: Felt252Wrapper,
    /// Fee paid for the transaction.
    pub actual_fee: Felt252Wrapper,
    /// Transaction type
    pub tx_type: TxType,
    /// Block Number
    pub block_number: u64,
    /// Block Hash
    pub block_hash: Felt252Wrapper,
    /// Messages sent in the transaction.
    // pub messages_sent: BoundedVec<Message, MaxArraySize>, // TODO: add messages
    /// Events emitted in the transaction.
    pub events: BoundedVec<EventWrapper, MaxArraySize>,
}

#[cfg(feature = "std")]
impl TransactionReceiptWrapper {
    /// Converts a [`TransactionReceiptWrapper`] to [`RPCMaybePendingTransactionReceipt`].
    ///
    /// This conversion is done in a function and not `From` trait due to the need
    /// to pass some arguments like the [`RPCTransactionStatus`] which is unknown
    /// in the [`TransactionReceiptWrapper`].
    ///
    /// Maybe extended later for other missing fields like messages sent to L1
    /// and the contract class for the deploy.
    pub fn into_maybe_pending_transaction_receipt(
        self,
        status: RPCTransactionStatus,
    ) -> RPCMaybePendingTransactionReceipt {
        let transaction_hash = self.transaction_hash.into();
        let actual_fee = self.actual_fee.into();
        let status = status;
        let block_hash = self.block_hash.into();
        let block_number = self.block_number;
        let events = self.events.iter().map(|e| (*e).clone().into()).collect();

        // TODO: from where those message must be taken?
        let messages_sent = vec![];

        match self.tx_type {
            TxType::DeployAccount => {
                RPCMaybePendingTransactionReceipt::Receipt(RPCTransactionReceipt::DeployAccount(
                    RPCDeployAccountTransactionReceipt {
                        transaction_hash,
                        actual_fee,
                        status,
                        block_hash,
                        block_number,
                        messages_sent,
                        events,
                        // TODO: from where can I get this one?
                        contract_address: FieldElement::ZERO,
                    },
                ))
            }
            TxType::Declare => RPCMaybePendingTransactionReceipt::Receipt(RPCTransactionReceipt::Declare(
                RPCDeclareTransactionReceipt {
                    transaction_hash,
                    actual_fee,
                    status,
                    block_hash,
                    block_number,
                    messages_sent,
                    events,
                },
            )),
            TxType::Invoke => {
                RPCMaybePendingTransactionReceipt::Receipt(RPCTransactionReceipt::Invoke(RPCInvokeTransactionReceipt {
                    transaction_hash,
                    actual_fee,
                    status,
                    block_hash,
                    block_number,
                    messages_sent,
                    events,
                }))
            }
            TxType::L1Handler => RPCMaybePendingTransactionReceipt::Receipt(RPCTransactionReceipt::L1Handler(
                RPCL1HandlerTransactionReceipt {
                    transaction_hash,
                    actual_fee,
                    status,
                    block_hash,
                    block_number,
                    messages_sent,
                    events,
                },
            )),
        }
    }
}

/// Representation of a Starknet event.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    scale_codec::Encode,
    scale_codec::Decode,
    scale_info::TypeInfo,
    scale_codec::MaxEncodedLen,
)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct EventWrapper {
    /// The keys (topics) of the event.
    pub keys: BoundedVec<Felt252Wrapper, MaxArraySize>,
    /// The data of the event.
    pub data: BoundedVec<Felt252Wrapper, MaxArraySize>,
    /// The address that emitted the event
    pub from_address: ContractAddressWrapper,
    /// The hash of the transaction that emitted the event
    pub transaction_hash: Felt252Wrapper,
}

#[cfg(feature = "std")]
impl From<EventWrapper> for RPCEvent {
    fn from(value: EventWrapper) -> Self {
        Self {
            from_address: value.from_address.into(),
            keys: value.keys.iter().map(|k| (*k).into()).collect(),
            data: value.data.iter().map(|d| (*d).into()).collect(),
        }
    }
}

/// This struct wraps the \[TransactionExecutionInfo\] type from the blockifier.
#[derive(Debug)]
pub struct TransactionExecutionInfoWrapper {
    /// Transaction validation call info; [None] for `L1Handler`.
    pub validate_call_info: Option<CallInfo>,
    /// Transaction execution call info; [None] for `Declare`.
    pub execute_call_info: Option<CallInfo>,
    /// Fee transfer call info; [None] for `L1Handler`.
    pub fee_transfer_call_info: Option<CallInfo>,
    /// The actual fee that was charged (in Wei).
    pub actual_fee: Fee,
    /// Actual execution resources the transaction is charged for,
    /// including L1 gas and additional OS resources estimation.
    pub actual_resources: BTreeMap<String, usize>,
}

/// Error enum wrapper for events.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    scale_codec::Encode,
    scale_codec::Decode,
    scale_info::TypeInfo,
    scale_codec::MaxEncodedLen,
    Error,
)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub enum EventError {
    /// Provided keys are invalid.
    #[error("Provided keys are invalid")]
    InvalidKeys,
    /// Provided data is invalid.
    #[error("Provided data is invalid")]
    InvalidData,
    /// Provided from address is invalid.
    #[error("Provided from address is invalid")]
    InvalidFromAddress,
    /// Too many events
    #[error("Too many events")]
    TooManyEvents,
}

/// Error enum wrapper for state diffs.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    scale_codec::Encode,
    scale_codec::Decode,
    scale_info::TypeInfo,
    scale_codec::MaxEncodedLen,
    Error,
)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub enum StateDiffError {
    /// Couldn't register newly deployed contracts.
    #[error("Couldn't register newly deployed contracts")]
    DeployedContractError,
    /// Couldn't register newly declared contracts.
    #[error("Couldn't register newly declared contracts")]
    DeclaredClassError,
}
