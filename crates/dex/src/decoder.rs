use crate::abi::{ISolidlyRouter, IUniswapV2Router02};
use alloy::primitives::{Address, U256};
use alloy::sol_types::SolCall;
use anyhow::Result;

#[derive(Debug, Clone)]
pub struct V2AddLiquidity {
    pub token_a: Address,
    pub token_b: Address,
    pub amount_a_desired: U256,
    pub amount_b_desired: U256,
    pub to: Address,
    pub deadline: U256,
}

#[derive(Debug, Clone)]
pub struct V2AddLiquidityEth {
    pub token: Address,
    pub amount_token_desired: U256,
    pub amount_token_min: U256,
    pub amount_eth_min: U256,
    pub to: Address,
    pub deadline: U256,
}

#[derive(Debug, Clone)]
pub struct SolidlyAddLiquidity {
    pub token_a: Address,
    pub token_b: Address,
    pub stable: bool,
    pub amount_a_desired: U256,
    pub amount_b_desired: U256,
    pub amount_a_min: U256,
    pub amount_b_min: U256,
    pub to: Address,
    pub deadline: U256,
}

#[derive(Debug, Clone)]
pub struct SolidlyAddLiquidityEth {
    pub token: Address,
    pub stable: bool,
    pub amount_token_desired: U256,
    pub amount_token_min: U256,
    pub amount_eth_min: U256,
    pub to: Address,
    pub deadline: U256,
}

#[derive(Debug, Clone)]
pub enum RouterCall {
    AddLiquidity(V2AddLiquidity),
    AddLiquidityEth(V2AddLiquidityEth),
    AddLiquiditySolidly(SolidlyAddLiquidity),
    AddLiquidityEthSolidly(SolidlyAddLiquidityEth),
}

pub fn decode_router_calldata(input: &[u8]) -> Result<Option<RouterCall>> {
    if input.len() < 4 {
        return Ok(None);
    }

    let selector = &input[..4];
    if selector == IUniswapV2Router02::addLiquidityCall::SELECTOR {
        let call = IUniswapV2Router02::addLiquidityCall::abi_decode(input)?;
        return Ok(Some(RouterCall::AddLiquidity(call.into())));
    }

    if selector == IUniswapV2Router02::addLiquidityETHCall::SELECTOR {
        let call = IUniswapV2Router02::addLiquidityETHCall::abi_decode(input)?;
        return Ok(Some(RouterCall::AddLiquidityEth(call.into())));
    }

    if selector == ISolidlyRouter::addLiquidityCall::SELECTOR {
        let call = ISolidlyRouter::addLiquidityCall::abi_decode(input)?;
        return Ok(Some(RouterCall::AddLiquiditySolidly(call.into())));
    }

    if selector == ISolidlyRouter::addLiquidityETHCall::SELECTOR {
        let call = ISolidlyRouter::addLiquidityETHCall::abi_decode(input)?;
        return Ok(Some(RouterCall::AddLiquidityEthSolidly(call.into())));
    }

    Ok(None)
}

impl From<IUniswapV2Router02::addLiquidityCall> for V2AddLiquidity {
    fn from(call: IUniswapV2Router02::addLiquidityCall) -> Self {
        Self {
            token_a: call.tokenA,
            token_b: call.tokenB,
            amount_a_desired: call.amountADesired,
            amount_b_desired: call.amountBDesired,
            to: call.to,
            deadline: call.deadline,
        }
    }
}

impl From<IUniswapV2Router02::addLiquidityETHCall> for V2AddLiquidityEth {
    fn from(call: IUniswapV2Router02::addLiquidityETHCall) -> Self {
        Self {
            token: call.token,
            amount_token_desired: call.amountTokenDesired,
            amount_token_min: call.amountTokenMin,
            amount_eth_min: call.amountETHMin,
            to: call.to,
            deadline: call.deadline,
        }
    }
}

impl From<ISolidlyRouter::addLiquidityCall> for SolidlyAddLiquidity {
    fn from(call: ISolidlyRouter::addLiquidityCall) -> Self {
        Self {
            token_a: call.tokenA,
            token_b: call.tokenB,
            stable: call.stable,
            amount_a_desired: call.amountADesired,
            amount_b_desired: call.amountBDesired,
            amount_a_min: call.amountAMin,
            amount_b_min: call.amountBMin,
            to: call.to,
            deadline: call.deadline,
        }
    }
}

impl From<ISolidlyRouter::addLiquidityETHCall> for SolidlyAddLiquidityEth {
    fn from(call: ISolidlyRouter::addLiquidityETHCall) -> Self {
        Self {
            token: call.token,
            stable: call.stable,
            amount_token_desired: call.amountTokenDesired,
            amount_token_min: call.amountTokenMin,
            amount_eth_min: call.amountETHMin,
            to: call.to,
            deadline: call.deadline,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    #[test]
    fn decode_add_liquidity() {
        let call = IUniswapV2Router02::addLiquidityCall {
            tokenA: address!("0x1000000000000000000000000000000000000001"),
            tokenB: address!("0x2000000000000000000000000000000000000002"),
            amountADesired: U256::from(1_000u64),
            amountBDesired: U256::from(2_000u64),
            amountAMin: U256::from(900u64),
            amountBMin: U256::from(1_800u64),
            to: address!("0x3000000000000000000000000000000000000003"),
            deadline: U256::from(123u64),
        };

        let data = call.abi_encode();
        let decoded = decode_router_calldata(&data).unwrap();
        match decoded {
            Some(RouterCall::AddLiquidity(add)) => {
                assert_eq!(add.token_a, call.tokenA);
                assert_eq!(add.token_b, call.tokenB);
                assert_eq!(add.amount_a_desired, call.amountADesired);
                assert_eq!(add.amount_b_desired, call.amountBDesired);
                assert_eq!(add.to, call.to);
                assert_eq!(add.deadline, call.deadline);
            }
            _ => panic!("unexpected decode"),
        }
    }

    #[test]
    fn decode_add_liquidity_eth() {
        let call = IUniswapV2Router02::addLiquidityETHCall {
            token: address!("0x1000000000000000000000000000000000000001"),
            amountTokenDesired: U256::from(1_000u64),
            amountTokenMin: U256::from(900u64),
            amountETHMin: U256::from(2_000u64),
            to: address!("0x3000000000000000000000000000000000000003"),
            deadline: U256::from(123u64),
        };

        let data = call.abi_encode();
        let decoded = decode_router_calldata(&data).unwrap();
        match decoded {
            Some(RouterCall::AddLiquidityEth(add)) => {
                assert_eq!(add.token, call.token);
                assert_eq!(add.amount_token_desired, call.amountTokenDesired);
                assert_eq!(add.amount_token_min, call.amountTokenMin);
                assert_eq!(add.amount_eth_min, call.amountETHMin);
                assert_eq!(add.to, call.to);
                assert_eq!(add.deadline, call.deadline);
            }
            _ => panic!("unexpected decode"),
        }
    }

    #[test]
    fn decode_solidly_add_liquidity() {
        let call = ISolidlyRouter::addLiquidityCall {
            tokenA: address!("0x1000000000000000000000000000000000000001"),
            tokenB: address!("0x2000000000000000000000000000000000000002"),
            stable: true,
            amountADesired: U256::from(1_000u64),
            amountBDesired: U256::from(2_000u64),
            amountAMin: U256::from(900u64),
            amountBMin: U256::from(1_800u64),
            to: address!("0x3000000000000000000000000000000000000003"),
            deadline: U256::from(123u64),
        };

        let data = call.abi_encode();
        let decoded = decode_router_calldata(&data).unwrap();
        match decoded {
            Some(RouterCall::AddLiquiditySolidly(add)) => {
                assert_eq!(add.token_a, call.tokenA);
                assert_eq!(add.token_b, call.tokenB);
                assert_eq!(add.stable, call.stable);
                assert_eq!(add.amount_a_desired, call.amountADesired);
                assert_eq!(add.amount_b_desired, call.amountBDesired);
                assert_eq!(add.amount_a_min, call.amountAMin);
                assert_eq!(add.amount_b_min, call.amountBMin);
                assert_eq!(add.to, call.to);
                assert_eq!(add.deadline, call.deadline);
            }
            _ => panic!("unexpected decode"),
        }
    }

    #[test]
    fn decode_solidly_add_liquidity_eth() {
        let call = ISolidlyRouter::addLiquidityETHCall {
            token: address!("0x1000000000000000000000000000000000000001"),
            stable: false,
            amountTokenDesired: U256::from(1_000u64),
            amountTokenMin: U256::from(900u64),
            amountETHMin: U256::from(2_000u64),
            to: address!("0x3000000000000000000000000000000000000003"),
            deadline: U256::from(123u64),
        };

        let data = call.abi_encode();
        let decoded = decode_router_calldata(&data).unwrap();
        match decoded {
            Some(RouterCall::AddLiquidityEthSolidly(add)) => {
                assert_eq!(add.token, call.token);
                assert_eq!(add.stable, call.stable);
                assert_eq!(add.amount_token_desired, call.amountTokenDesired);
                assert_eq!(add.amount_token_min, call.amountTokenMin);
                assert_eq!(add.amount_eth_min, call.amountETHMin);
                assert_eq!(add.to, call.to);
                assert_eq!(add.deadline, call.deadline);
            }
            _ => panic!("unexpected decode"),
        }
    }
}
