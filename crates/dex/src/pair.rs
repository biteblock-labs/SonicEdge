use crate::abi::{
    IFactoryInitCodeHash,
    IFactoryPairCodeHash,
    ISolidlyFactory,
    IUniswapV2Factory,
    IUniswapV2Pair,
};
use alloy::eips::BlockId;
use alloy::primitives::{keccak256, Address, B256, TxKind, U256};
use alloy::providers::{DynProvider, Provider};
use alloy::rpc::types::transaction::TransactionInput;
use alloy::rpc::types::TransactionRequest;
use alloy::sol_types::SolCall;
use anyhow::Result;

pub async fn get_pair_address(
    provider: &DynProvider,
    factory: Address,
    token_a: Address,
    token_b: Address,
) -> Result<Option<Address>> {
    let call = IUniswapV2Factory::getPairCall {
        tokenA: token_a,
        tokenB: token_b,
    };
    let tx = TransactionRequest {
        to: Some(TxKind::Call(factory)),
        input: TransactionInput::new(call.abi_encode().into()),
        ..Default::default()
    };
    let data = provider.call(tx).await?;
    let ret = IUniswapV2Factory::getPairCall::abi_decode_returns(&data)?;
    if ret == Address::ZERO {
        return Ok(None);
    }
    Ok(Some(ret))
}

pub async fn get_pair_address_solidly(
    provider: &DynProvider,
    factory: Address,
    token_a: Address,
    token_b: Address,
    stable: bool,
) -> Result<Option<Address>> {
    let call = ISolidlyFactory::getPairCall {
        tokenA: token_a,
        tokenB: token_b,
        stable,
    };
    let tx = TransactionRequest {
        to: Some(TxKind::Call(factory)),
        input: TransactionInput::new(call.abi_encode().into()),
        ..Default::default()
    };
    let data = provider.call(tx).await?;
    let ret = ISolidlyFactory::getPairCall::abi_decode_returns(&data)?;
    if ret == Address::ZERO {
        return Ok(None);
    }
    Ok(Some(ret))
}

pub async fn get_pair_code_hash(provider: &DynProvider, factory: Address) -> Result<Option<B256>> {
    if let Ok(hash) = call_pair_code_hash(provider, factory).await {
        return Ok(Some(hash));
    }
    if let Ok(hash) = call_init_code_hash(provider, factory).await {
        return Ok(Some(hash));
    }
    Ok(None)
}

pub async fn contract_exists(provider: &DynProvider, address: Address) -> Result<bool> {
    let code = provider.get_code_at(address).await?;
    Ok(!code.is_empty())
}

pub async fn contract_exists_at_block(
    provider: &DynProvider,
    address: Address,
    block_number: u64,
) -> Result<bool> {
    let code = provider
        .get_code_at(address)
        .block_id(BlockId::number(block_number))
        .await?;
    Ok(!code.is_empty())
}

pub fn derive_pair_address_v2(
    factory: Address,
    token_a: Address,
    token_b: Address,
    init_code_hash: B256,
) -> Address {
    let (token0, token1) = sort_tokens(token_a, token_b);
    let mut packed = [0u8; 40];
    packed[..20].copy_from_slice(token0.0.as_ref());
    packed[20..].copy_from_slice(token1.0.as_ref());
    let salt = keccak256(packed);
    factory.create2(salt, init_code_hash)
}

pub fn derive_pair_address_solidly(
    factory: Address,
    token_a: Address,
    token_b: Address,
    stable: bool,
    init_code_hash: B256,
) -> Address {
    let (token0, token1) = sort_tokens(token_a, token_b);
    let mut packed = [0u8; 41];
    packed[..20].copy_from_slice(token0.0.as_ref());
    packed[20..40].copy_from_slice(token1.0.as_ref());
    packed[40] = if stable { 1 } else { 0 };
    let salt = keccak256(packed);
    factory.create2(salt, init_code_hash)
}

pub async fn get_reserves(provider: &DynProvider, pair: Address) -> Result<Option<(U256, U256)>> {
    let call = IUniswapV2Pair::getReservesCall {};
    let tx = TransactionRequest {
        to: Some(TxKind::Call(pair)),
        input: TransactionInput::new(call.abi_encode().into()),
        ..Default::default()
    };
    let data = provider.call(tx).await?;
    let ret = IUniswapV2Pair::getReservesCall::abi_decode_returns(&data)?;
    Ok(Some((U256::from(ret.reserve0), U256::from(ret.reserve1))))
}

pub async fn get_reserves_at_block(
    provider: &DynProvider,
    pair: Address,
    block_number: u64,
) -> Result<Option<(U256, U256)>> {
    let call = IUniswapV2Pair::getReservesCall {};
    let tx = TransactionRequest {
        to: Some(TxKind::Call(pair)),
        input: TransactionInput::new(call.abi_encode().into()),
        ..Default::default()
    };
    let data = provider
        .call(tx)
        .block(BlockId::number(block_number))
        .await?;
    let ret = IUniswapV2Pair::getReservesCall::abi_decode_returns(&data)?;
    Ok(Some((U256::from(ret.reserve0), U256::from(ret.reserve1))))
}

pub async fn get_pair_tokens(
    provider: &DynProvider,
    pair: Address,
) -> Result<Option<(Address, Address)>> {
    let call0 = IUniswapV2Pair::token0Call {};
    let tx0 = TransactionRequest {
        to: Some(TxKind::Call(pair)),
        input: TransactionInput::new(call0.abi_encode().into()),
        ..Default::default()
    };
    let data0 = provider.call(tx0).await?;
    let token0 = IUniswapV2Pair::token0Call::abi_decode_returns(&data0)?;

    let call1 = IUniswapV2Pair::token1Call {};
    let tx1 = TransactionRequest {
        to: Some(TxKind::Call(pair)),
        input: TransactionInput::new(call1.abi_encode().into()),
        ..Default::default()
    };
    let data1 = provider.call(tx1).await?;
    let token1 = IUniswapV2Pair::token1Call::abi_decode_returns(&data1)?;

    if token0 == Address::ZERO || token1 == Address::ZERO {
        return Ok(None);
    }

    Ok(Some((token0, token1)))
}

fn sort_tokens(token_a: Address, token_b: Address) -> (Address, Address) {
    if token_a < token_b {
        (token_a, token_b)
    } else {
        (token_b, token_a)
    }
}

async fn call_pair_code_hash(provider: &DynProvider, factory: Address) -> Result<B256> {
    let call = IFactoryPairCodeHash::pairCodeHashCall {};
    let tx = TransactionRequest {
        to: Some(TxKind::Call(factory)),
        input: TransactionInput::new(call.abi_encode().into()),
        ..Default::default()
    };
    let data = provider.call(tx).await?;
    let ret = IFactoryPairCodeHash::pairCodeHashCall::abi_decode_returns(&data)?;
    Ok(ret)
}

async fn call_init_code_hash(provider: &DynProvider, factory: Address) -> Result<B256> {
    let call = IFactoryInitCodeHash::INIT_CODE_PAIR_HASHCall {};
    let tx = TransactionRequest {
        to: Some(TxKind::Call(factory)),
        input: TransactionInput::new(call.abi_encode().into()),
        ..Default::default()
    };
    let data = provider.call(tx).await?;
    let ret = IFactoryInitCodeHash::INIT_CODE_PAIR_HASHCall::abi_decode_returns(&data)?;
    Ok(ret)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{address, b256};

    #[test]
    fn derive_pair_address_v2_matches_expected() {
        let factory = address!("0x1000000000000000000000000000000000000000");
        let token_a = address!("0x00000000000000000000000000000000000000aa");
        let token_b = address!("0x00000000000000000000000000000000000000bb");
        let init_code_hash =
            b256!("0x0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef");
        let expected = address!("0x3df2116cc026cc038106f7a39c2d308e5dd5d06a");

        assert_eq!(
            derive_pair_address_v2(factory, token_a, token_b, init_code_hash),
            expected
        );
        assert_eq!(
            derive_pair_address_v2(factory, token_b, token_a, init_code_hash),
            expected
        );
    }

    #[test]
    fn derive_pair_address_solidly_includes_stable_flag() {
        let factory = address!("0x1000000000000000000000000000000000000000");
        let token_a = address!("0x00000000000000000000000000000000000000aa");
        let token_b = address!("0x00000000000000000000000000000000000000bb");
        let init_code_hash =
            b256!("0x0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef");
        let expected_false = address!("0x0a95061d11bd5623a03865940d71e0dcf3f135cb");
        let expected_true = address!("0x6070ee43e915f087e91fddc432b2d5017246d064");

        assert_eq!(
            derive_pair_address_solidly(factory, token_a, token_b, false, init_code_hash),
            expected_false
        );
        assert_eq!(
            derive_pair_address_solidly(factory, token_b, token_a, false, init_code_hash),
            expected_false
        );
        assert_eq!(
            derive_pair_address_solidly(factory, token_a, token_b, true, init_code_hash),
            expected_true
        );
        assert_ne!(expected_false, expected_true);
    }
}
