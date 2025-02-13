// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: 2023 Snowfork <hello@snowfork.com>
//! Governance API for controlling the Ethereum side of the bridge
//!
//! # Extrinsics
//!
//! ## Agents
//!
//! Agents are smart contracts on Ethereum that act as proxies for consensus systems on Polkadot
//! networks.
//!
//! * [`Call::create_agent`]: Create agent for any kind of sovereign location on Polkadot network,
//!   can be a sibling parachain, pallet or smart contract or signed account in that parachain, etc

//! ## Polkadot-native tokens on Ethereum
//!
//! Tokens deposited on AssetHub pallet can be bridged to Ethereum as wrapped ERC20 tokens. As a
//! prerequisite, the token should be registered first.
//!
//! * [`Call::register_token`]: Register a token location as a wrapped ERC20 contract on Ethereum.
#![cfg_attr(not(feature = "std"), no_std)]
#[cfg(test)]
mod mock;

#[cfg(test)]
mod tests;

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;

pub mod api;
pub mod weights;
pub use weights::*;

use frame_support::{pallet_prelude::*, traits::EnsureOrigin};
use frame_system::pallet_prelude::*;
use snowbridge_core::{AgentId, AgentIdOf, AssetMetadata, TokenId, TokenIdOf};
use snowbridge_outbound_queue_primitives::{
	v2::{Command, Initializer, Message, SendMessage},
	OperatingMode, SendError,
};
use sp_core::{H160, H256};
use sp_io::hashing::blake2_256;
use sp_runtime::traits::MaybeEquivalence;
use sp_std::prelude::*;
use xcm::prelude::*;
use xcm_executor::traits::ConvertLocation;

#[cfg(feature = "runtime-benchmarks")]
use frame_support::traits::OriginTrait;

pub use pallet::*;

pub type AccountIdOf<T> = <T as frame_system::Config>::AccountId;

pub fn agent_id_of<T: Config>(location: &Location) -> Result<H256, DispatchError> {
	T::AgentIdOf::convert_location(location).ok_or(Error::<T>::LocationConversionFailed.into())
}

#[cfg(feature = "runtime-benchmarks")]
pub trait BenchmarkHelper<O>
where
	O: OriginTrait,
{
	fn make_xcm_origin(location: Location) -> O;
}

#[frame_support::pallet]
pub mod pallet {
	use super::*;

	#[pallet::pallet]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: frame_system::Config {
		type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;

		/// Send messages to Ethereum
		type OutboundQueue: SendMessage;

		/// Origin check for XCM locations that transact with this pallet
		type SiblingOrigin: EnsureOrigin<Self::RuntimeOrigin, Success = Location>;

		/// Converts Location to AgentId
		type AgentIdOf: ConvertLocation<AgentId>;

		/// This chain's Universal Location.
		type UniversalLocation: Get<InteriorLocation>;

		/// The bridges configured Ethereum location
		type EthereumLocation: Get<Location>;

		type WeightInfo: WeightInfo;
		#[cfg(feature = "runtime-benchmarks")]
		type Helper: BenchmarkHelper<Self::RuntimeOrigin>;
	}

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		/// An CreateAgent message was sent to the Gateway
		CreateAgent { location: Box<Location>, agent_id: AgentId },
		/// Register Polkadot-native token as a wrapped ERC20 token on Ethereum
		RegisterToken {
			/// Location of Polkadot-native token
			location: VersionedLocation,
			/// ID of Polkadot-native token on Ethereum
			foreign_token_id: H256,
		},
		/// An Upgrade message was sent to the Gateway
		Upgrade { impl_address: H160, impl_code_hash: H256, initializer_params_hash: Option<H256> },
		/// An SetOperatingMode message was sent to the Gateway
		SetOperatingMode { mode: OperatingMode },
	}

	#[pallet::error]
	pub enum Error<T> {
		LocationConversionFailed,
		AgentAlreadyCreated,
		NoAgent,
		UnsupportedLocationVersion,
		InvalidLocation,
		Send(SendError),
		InvalidUpgradeParameters,
	}

	/// The set of registered agents
	#[pallet::storage]
	#[pallet::getter(fn agents)]
	pub type Agents<T: Config> = StorageMap<_, Twox64Concat, AgentId, (), OptionQuery>;

	/// Lookup table for foreign token ID to native location relative to ethereum
	#[pallet::storage]
	pub type ForeignToNativeId<T: Config> =
		StorageMap<_, Blake2_128Concat, TokenId, xcm::v5::Location, OptionQuery>;

	/// Lookup table for native location relative to ethereum to foreign token ID
	#[pallet::storage]
	pub type NativeToForeignId<T: Config> =
		StorageMap<_, Blake2_128Concat, xcm::v5::Location, TokenId, OptionQuery>;

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Sends a command to the Gateway contract to instantiate a new agent contract representing
		/// `origin`.
		///
		/// - `location`: The location representing the agent
		/// - `fee`: Ether to pay for the execution cost on Ethereum
		#[pallet::call_index(1)]
		#[pallet::weight(T::WeightInfo::create_agent())]
		pub fn create_agent(
			origin: OriginFor<T>,
			location: Box<VersionedLocation>,
			fee: u128,
		) -> DispatchResult {
			T::SiblingOrigin::ensure_origin(origin)?;

			let origin_location: Location =
				(*location).try_into().map_err(|_| Error::<T>::UnsupportedLocationVersion)?;

			// reanchor to Ethereum context
			let ethereum_location = T::EthereumLocation::get();
			let reanchored_location = origin_location
				.clone()
				.reanchored(&ethereum_location, &T::UniversalLocation::get())
				.map_err(|_| Error::<T>::LocationConversionFailed)?;

			let agent_id = agent_id_of::<T>(&reanchored_location)?;

			// Record the agent id or fail if it has already been created
			ensure!(!Agents::<T>::contains_key(agent_id), Error::<T>::AgentAlreadyCreated);
			Agents::<T>::insert(agent_id, ());

			let command = Command::CreateAgent {};

			Self::send(agent_id, command, fee)?;

			Self::deposit_event(Event::<T>::CreateAgent {
				location: Box::new(origin_location),
				agent_id,
			});
			Ok(())
		}

		/// Registers a Polkadot-native token as a wrapped ERC20 token on Ethereum.
		///
		/// - `asset_id`: Location of the asset (relative to this chain)
		/// - `metadata`: Metadata to include in the instantiated ERC20 contract on Ethereum
		/// - `fee`: Ether to pay for the execution cost on Ethereum
		#[pallet::call_index(2)]
		#[pallet::weight(T::WeightInfo::register_token())]
		pub fn register_token(
			origin: OriginFor<T>,
			asset_id: Box<VersionedLocation>,
			metadata: AssetMetadata,
			fee: u128,
		) -> DispatchResult {
			let origin_location = T::SiblingOrigin::ensure_origin(origin)?;
			let origin = AgentIdOf::convert_location(&origin_location)
				.ok_or(Error::<T>::LocationConversionFailed)?;

			let asset_location: Location =
				(*asset_id).try_into().map_err(|_| Error::<T>::UnsupportedLocationVersion)?;

			let ethereum_location = T::EthereumLocation::get();
			// reanchor to Ethereum context
			let location = asset_location
				.clone()
				.reanchored(&ethereum_location, &T::UniversalLocation::get())
				.map_err(|_| Error::<T>::LocationConversionFailed)?;

			let token_id = TokenIdOf::convert_location(&location)
				.ok_or(Error::<T>::LocationConversionFailed)?;

			if !ForeignToNativeId::<T>::contains_key(token_id) {
				NativeToForeignId::<T>::insert(location.clone(), token_id);
				ForeignToNativeId::<T>::insert(token_id, location.clone());
			}

			let command = Command::RegisterForeignToken {
				token_id,
				name: metadata.name.into_inner(),
				symbol: metadata.symbol.into_inner(),
				decimals: metadata.decimals,
			};
			Self::send(origin, command, fee)?;

			Self::deposit_event(Event::<T>::RegisterToken {
				location: location.clone().into(),
				foreign_token_id: token_id,
			});

			Ok(())
		}

		/// Sends command to the Gateway contract to upgrade itself with a new implementation
		/// contract
		///
		/// Fee required: No
		///
		/// - `origin`: Must be `Root`.
		/// - `impl_address`: The address of the implementation contract.
		/// - `impl_code_hash`: The codehash of the implementation contract.
		/// - `initializer`: Optionally call an initializer on the implementation contract.
		#[pallet::call_index(3)]
		#[pallet::weight((T::WeightInfo::upgrade(), DispatchClass::Operational))]
		pub fn upgrade(
			origin: OriginFor<T>,
			impl_address: H160,
			impl_code_hash: H256,
			initializer: Option<Initializer>,
		) -> DispatchResult {
			ensure_root(origin)?;

			let origin = Self::governance_origin()?;

			ensure!(
				!impl_address.eq(&H160::zero()) && !impl_code_hash.eq(&H256::zero()),
				Error::<T>::InvalidUpgradeParameters
			);

			let initializer_params_hash: Option<H256> =
				initializer.as_ref().map(|i| H256::from(blake2_256(i.params.as_ref())));

			let command = Command::Upgrade { impl_address, impl_code_hash, initializer };
			Self::send(origin, command, 0)?;

			Self::deposit_event(Event::<T>::Upgrade {
				impl_address,
				impl_code_hash,
				initializer_params_hash,
			});
			Ok(())
		}

		/// Sends a message to the Gateway contract to change its operating mode
		///
		/// Fee required: No
		///
		/// - `origin`: Must be `Root`
		#[pallet::call_index(4)]
		#[pallet::weight((T::WeightInfo::set_operating_mode(), DispatchClass::Operational))]
		pub fn set_operating_mode(origin: OriginFor<T>, mode: OperatingMode) -> DispatchResult {
			ensure_root(origin)?;

			let origin = Self::governance_origin()?;

			let command = Command::SetOperatingMode { mode };
			Self::send(origin, command, 0)?;

			Self::deposit_event(Event::<T>::SetOperatingMode { mode });
			Ok(())
		}
	}

	impl<T: Config> Pallet<T> {
		/// Send `command` to the Gateway from a specific origin/agent
		fn send(origin: AgentId, command: Command, fee: u128) -> DispatchResult {
			let mut message = Message {
				origin,
				id: Default::default(),
				fee,
				commands: BoundedVec::try_from(vec![command]).unwrap(),
			};
			let hash = sp_io::hashing::blake2_256(&message.encode());
			message.id = hash.into();

			let (ticket, _) =
				T::OutboundQueue::validate(&message).map_err(|err| Error::<T>::Send(err))?;

			T::OutboundQueue::deliver(ticket).map_err(|err| Error::<T>::Send(err))?;
			Ok(())
		}

		fn governance_origin() -> Result<AgentId, Error<T>> {
			AgentIdOf::convert_location(&Here.into()).ok_or(Error::<T>::LocationConversionFailed)
		}
	}

	impl<T: Config> MaybeEquivalence<TokenId, Location> for Pallet<T> {
		fn convert(foreign_id: &TokenId) -> Option<Location> {
			ForeignToNativeId::<T>::get(foreign_id)
		}
		fn convert_back(location: &Location) -> Option<TokenId> {
			NativeToForeignId::<T>::get(location)
		}
	}
}
