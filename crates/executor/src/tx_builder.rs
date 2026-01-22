use crate::abi::SonicSniperExecutor;
use crate::fees::FeeStrategy;
use alloy::primitives::{Address, TxKind, U256};
use alloy::rpc::types::transaction::TransactionInput;
use alloy::rpc::types::TransactionRequest;
use alloy::sol_types::SolCall;

type U112 = alloy::primitives::Uint<112, 2>;

#[derive(Debug, Clone)]
pub struct BuyV2Params {
    pub router: Address,
    pub path: Vec<Address>,
    pub amount_in: U256,
    pub min_amount_out: U256,
    pub recipient: Address,
    pub deadline: U256,
    pub pair: Address,
    pub min_base_reserve: u128,
    pub min_token_reserve: u128,
    pub max_block_number: u64,
}

#[derive(Debug, Clone)]
pub struct BuySolidlyParams {
    pub router: Address,
    pub token_in: Address,
    pub token_out: Address,
    pub stable: bool,
    pub amount_in: U256,
    pub min_amount_out: U256,
    pub recipient: Address,
    pub deadline: U256,
    pub pair: Address,
    pub min_base_reserve: u128,
    pub min_token_reserve: u128,
    pub max_block_number: u64,
}

#[derive(Debug, Clone)]
pub struct BuyV2EthParams {
    pub router: Address,
    pub path: Vec<Address>,
    pub amount_in: U256,
    pub min_amount_out: U256,
    pub recipient: Address,
    pub deadline: U256,
    pub pair: Address,
    pub min_base_reserve: u128,
    pub min_token_reserve: u128,
    pub max_block_number: u64,
}

#[derive(Debug, Clone)]
pub struct BuySolidlyEthParams {
    pub router: Address,
    pub token_in: Address,
    pub token_out: Address,
    pub stable: bool,
    pub amount_in: U256,
    pub min_amount_out: U256,
    pub recipient: Address,
    pub deadline: U256,
    pub pair: Address,
    pub min_base_reserve: u128,
    pub min_token_reserve: u128,
    pub max_block_number: u64,
}

#[derive(Debug, Clone)]
pub struct SellV2Params {
    pub router: Address,
    pub path: Vec<Address>,
    pub amount_in: U256,
    pub min_amount_out: U256,
    pub recipient: Address,
    pub deadline: U256,
    pub pair: Address,
    pub min_base_reserve: u128,
    pub min_token_reserve: u128,
    pub max_block_number: u64,
}

#[derive(Debug, Clone)]
pub struct SellV2EthParams {
    pub router: Address,
    pub path: Vec<Address>,
    pub amount_in: U256,
    pub min_amount_out: U256,
    pub recipient: Address,
    pub deadline: U256,
    pub pair: Address,
    pub min_base_reserve: u128,
    pub min_token_reserve: u128,
    pub max_block_number: u64,
}

#[derive(Debug, Clone)]
pub struct SellSolidlyParams {
    pub router: Address,
    pub token_in: Address,
    pub token_out: Address,
    pub stable: bool,
    pub amount_in: U256,
    pub min_amount_out: U256,
    pub recipient: Address,
    pub deadline: U256,
    pub pair: Address,
    pub min_base_reserve: u128,
    pub min_token_reserve: u128,
    pub max_block_number: u64,
}

#[derive(Debug, Clone)]
pub struct SellSolidlyEthParams {
    pub router: Address,
    pub token_in: Address,
    pub token_out: Address,
    pub stable: bool,
    pub amount_in: U256,
    pub min_amount_out: U256,
    pub recipient: Address,
    pub deadline: U256,
    pub pair: Address,
    pub min_base_reserve: u128,
    pub min_token_reserve: u128,
    pub max_block_number: u64,
}

#[derive(Clone)]
pub struct ExecutorTxBuilder {
    pub contract: Address,
    pub owner: Address,
    pub chain_id: u64,
    pub fees: FeeStrategy,
}

impl ExecutorTxBuilder {
    pub fn new(contract: Address, owner: Address, chain_id: u64, fees: FeeStrategy) -> Self {
        Self {
            contract,
            owner,
            chain_id,
            fees,
        }
    }

    pub fn build_buy_v2(&self, params: BuyV2Params, nonce: u64) -> TransactionRequest {
        let call = SonicSniperExecutor::buyV2Call {
            router: params.router,
            path: params.path,
            amountIn: params.amount_in,
            minAmountOut: params.min_amount_out,
            recipient: params.recipient,
            deadline: params.deadline,
            pair: params.pair,
            minBaseReserve: U112::from(params.min_base_reserve),
            minTokenReserve: U112::from(params.min_token_reserve),
            maxBlockNumber: params.max_block_number,
        };

        let mut tx = TransactionRequest {
            from: Some(self.owner),
            to: Some(TxKind::Call(self.contract)),
            input: TransactionInput::new(call.abi_encode().into()),
            nonce: Some(nonce),
            chain_id: Some(self.chain_id),
            ..Default::default()
        };
        self.fees.apply(&mut tx);
        tx
    }

    pub fn build_buy_v2_eth(&self, params: BuyV2EthParams, nonce: u64) -> TransactionRequest {
        let call = SonicSniperExecutor::buyV2ETHCall {
            router: params.router,
            path: params.path,
            amountIn: params.amount_in,
            minAmountOut: params.min_amount_out,
            recipient: params.recipient,
            deadline: params.deadline,
            pair: params.pair,
            minBaseReserve: U112::from(params.min_base_reserve),
            minTokenReserve: U112::from(params.min_token_reserve),
            maxBlockNumber: params.max_block_number,
        };

        let mut tx = TransactionRequest {
            from: Some(self.owner),
            to: Some(TxKind::Call(self.contract)),
            input: TransactionInput::new(call.abi_encode().into()),
            value: Some(params.amount_in),
            nonce: Some(nonce),
            chain_id: Some(self.chain_id),
            ..Default::default()
        };
        self.fees.apply(&mut tx);
        tx
    }

    pub fn build_buy_solidly(
        &self,
        params: BuySolidlyParams,
        nonce: u64,
    ) -> TransactionRequest {
        let call = SonicSniperExecutor::buySolidlyCall {
            router: params.router,
            tokenIn: params.token_in,
            tokenOut: params.token_out,
            stable: params.stable,
            amountIn: params.amount_in,
            minAmountOut: params.min_amount_out,
            recipient: params.recipient,
            deadline: params.deadline,
            pair: params.pair,
            minBaseReserve: U112::from(params.min_base_reserve),
            minTokenReserve: U112::from(params.min_token_reserve),
            maxBlockNumber: params.max_block_number,
        };

        let mut tx = TransactionRequest {
            from: Some(self.owner),
            to: Some(TxKind::Call(self.contract)),
            input: TransactionInput::new(call.abi_encode().into()),
            nonce: Some(nonce),
            chain_id: Some(self.chain_id),
            ..Default::default()
        };
        self.fees.apply(&mut tx);
        tx
    }

    pub fn build_buy_solidly_eth(
        &self,
        params: BuySolidlyEthParams,
        nonce: u64,
    ) -> TransactionRequest {
        let call = SonicSniperExecutor::buySolidlyETHCall {
            router: params.router,
            tokenIn: params.token_in,
            tokenOut: params.token_out,
            stable: params.stable,
            amountIn: params.amount_in,
            minAmountOut: params.min_amount_out,
            recipient: params.recipient,
            deadline: params.deadline,
            pair: params.pair,
            minBaseReserve: U112::from(params.min_base_reserve),
            minTokenReserve: U112::from(params.min_token_reserve),
            maxBlockNumber: params.max_block_number,
        };

        let mut tx = TransactionRequest {
            from: Some(self.owner),
            to: Some(TxKind::Call(self.contract)),
            input: TransactionInput::new(call.abi_encode().into()),
            value: Some(params.amount_in),
            nonce: Some(nonce),
            chain_id: Some(self.chain_id),
            ..Default::default()
        };
        self.fees.apply(&mut tx);
        tx
    }

    pub fn build_sell_v2(&self, params: SellV2Params, nonce: u64) -> TransactionRequest {
        let call = SonicSniperExecutor::sellV2Call {
            router: params.router,
            path: params.path,
            amountIn: params.amount_in,
            minAmountOut: params.min_amount_out,
            recipient: params.recipient,
            deadline: params.deadline,
            pair: params.pair,
            minBaseReserve: U112::from(params.min_base_reserve),
            minTokenReserve: U112::from(params.min_token_reserve),
            maxBlockNumber: params.max_block_number,
        };

        let mut tx = TransactionRequest {
            from: Some(self.owner),
            to: Some(TxKind::Call(self.contract)),
            input: TransactionInput::new(call.abi_encode().into()),
            nonce: Some(nonce),
            chain_id: Some(self.chain_id),
            ..Default::default()
        };
        self.fees.apply(&mut tx);
        tx
    }

    pub fn build_sell_v2_eth(&self, params: SellV2EthParams, nonce: u64) -> TransactionRequest {
        let call = SonicSniperExecutor::sellV2ETHCall {
            router: params.router,
            path: params.path,
            amountIn: params.amount_in,
            minAmountOut: params.min_amount_out,
            recipient: params.recipient,
            deadline: params.deadline,
            pair: params.pair,
            minBaseReserve: U112::from(params.min_base_reserve),
            minTokenReserve: U112::from(params.min_token_reserve),
            maxBlockNumber: params.max_block_number,
        };

        let mut tx = TransactionRequest {
            from: Some(self.owner),
            to: Some(TxKind::Call(self.contract)),
            input: TransactionInput::new(call.abi_encode().into()),
            nonce: Some(nonce),
            chain_id: Some(self.chain_id),
            ..Default::default()
        };
        self.fees.apply(&mut tx);
        tx
    }

    pub fn build_sell_solidly(
        &self,
        params: SellSolidlyParams,
        nonce: u64,
    ) -> TransactionRequest {
        let call = SonicSniperExecutor::sellSolidlyCall {
            router: params.router,
            tokenIn: params.token_in,
            tokenOut: params.token_out,
            stable: params.stable,
            amountIn: params.amount_in,
            minAmountOut: params.min_amount_out,
            recipient: params.recipient,
            deadline: params.deadline,
            pair: params.pair,
            minBaseReserve: U112::from(params.min_base_reserve),
            minTokenReserve: U112::from(params.min_token_reserve),
            maxBlockNumber: params.max_block_number,
        };

        let mut tx = TransactionRequest {
            from: Some(self.owner),
            to: Some(TxKind::Call(self.contract)),
            input: TransactionInput::new(call.abi_encode().into()),
            nonce: Some(nonce),
            chain_id: Some(self.chain_id),
            ..Default::default()
        };
        self.fees.apply(&mut tx);
        tx
    }

    pub fn build_sell_solidly_eth(
        &self,
        params: SellSolidlyEthParams,
        nonce: u64,
    ) -> TransactionRequest {
        let call = SonicSniperExecutor::sellSolidlyETHCall {
            router: params.router,
            tokenIn: params.token_in,
            tokenOut: params.token_out,
            stable: params.stable,
            amountIn: params.amount_in,
            minAmountOut: params.min_amount_out,
            recipient: params.recipient,
            deadline: params.deadline,
            pair: params.pair,
            minBaseReserve: U112::from(params.min_base_reserve),
            minTokenReserve: U112::from(params.min_token_reserve),
            maxBlockNumber: params.max_block_number,
        };

        let mut tx = TransactionRequest {
            from: Some(self.owner),
            to: Some(TxKind::Call(self.contract)),
            input: TransactionInput::new(call.abi_encode().into()),
            nonce: Some(nonce),
            chain_id: Some(self.chain_id),
            ..Default::default()
        };
        self.fees.apply(&mut tx);
        tx
    }

    pub fn encode_buy_v2(params: BuyV2Params) -> Vec<u8> {
        let call = SonicSniperExecutor::buyV2Call {
            router: params.router,
            path: params.path,
            amountIn: params.amount_in,
            minAmountOut: params.min_amount_out,
            recipient: params.recipient,
            deadline: params.deadline,
            pair: params.pair,
            minBaseReserve: U112::from(params.min_base_reserve),
            minTokenReserve: U112::from(params.min_token_reserve),
            maxBlockNumber: params.max_block_number,
        };
        call.abi_encode()
    }

    pub fn encode_buy_v2_eth(params: BuyV2EthParams) -> Vec<u8> {
        let call = SonicSniperExecutor::buyV2ETHCall {
            router: params.router,
            path: params.path,
            amountIn: params.amount_in,
            minAmountOut: params.min_amount_out,
            recipient: params.recipient,
            deadline: params.deadline,
            pair: params.pair,
            minBaseReserve: U112::from(params.min_base_reserve),
            minTokenReserve: U112::from(params.min_token_reserve),
            maxBlockNumber: params.max_block_number,
        };
        call.abi_encode()
    }

    pub fn encode_buy_solidly(params: BuySolidlyParams) -> Vec<u8> {
        let call = SonicSniperExecutor::buySolidlyCall {
            router: params.router,
            tokenIn: params.token_in,
            tokenOut: params.token_out,
            stable: params.stable,
            amountIn: params.amount_in,
            minAmountOut: params.min_amount_out,
            recipient: params.recipient,
            deadline: params.deadline,
            pair: params.pair,
            minBaseReserve: U112::from(params.min_base_reserve),
            minTokenReserve: U112::from(params.min_token_reserve),
            maxBlockNumber: params.max_block_number,
        };
        call.abi_encode()
    }

    pub fn encode_buy_solidly_eth(params: BuySolidlyEthParams) -> Vec<u8> {
        let call = SonicSniperExecutor::buySolidlyETHCall {
            router: params.router,
            tokenIn: params.token_in,
            tokenOut: params.token_out,
            stable: params.stable,
            amountIn: params.amount_in,
            minAmountOut: params.min_amount_out,
            recipient: params.recipient,
            deadline: params.deadline,
            pair: params.pair,
            minBaseReserve: U112::from(params.min_base_reserve),
            minTokenReserve: U112::from(params.min_token_reserve),
            maxBlockNumber: params.max_block_number,
        };
        call.abi_encode()
    }

    pub fn encode_sell_v2(params: SellV2Params) -> Vec<u8> {
        let call = SonicSniperExecutor::sellV2Call {
            router: params.router,
            path: params.path,
            amountIn: params.amount_in,
            minAmountOut: params.min_amount_out,
            recipient: params.recipient,
            deadline: params.deadline,
            pair: params.pair,
            minBaseReserve: U112::from(params.min_base_reserve),
            minTokenReserve: U112::from(params.min_token_reserve),
            maxBlockNumber: params.max_block_number,
        };
        call.abi_encode()
    }

    pub fn encode_sell_v2_eth(params: SellV2EthParams) -> Vec<u8> {
        let call = SonicSniperExecutor::sellV2ETHCall {
            router: params.router,
            path: params.path,
            amountIn: params.amount_in,
            minAmountOut: params.min_amount_out,
            recipient: params.recipient,
            deadline: params.deadline,
            pair: params.pair,
            minBaseReserve: U112::from(params.min_base_reserve),
            minTokenReserve: U112::from(params.min_token_reserve),
            maxBlockNumber: params.max_block_number,
        };
        call.abi_encode()
    }

    pub fn encode_sell_solidly(params: SellSolidlyParams) -> Vec<u8> {
        let call = SonicSniperExecutor::sellSolidlyCall {
            router: params.router,
            tokenIn: params.token_in,
            tokenOut: params.token_out,
            stable: params.stable,
            amountIn: params.amount_in,
            minAmountOut: params.min_amount_out,
            recipient: params.recipient,
            deadline: params.deadline,
            pair: params.pair,
            minBaseReserve: U112::from(params.min_base_reserve),
            minTokenReserve: U112::from(params.min_token_reserve),
            maxBlockNumber: params.max_block_number,
        };
        call.abi_encode()
    }

    pub fn encode_sell_solidly_eth(params: SellSolidlyEthParams) -> Vec<u8> {
        let call = SonicSniperExecutor::sellSolidlyETHCall {
            router: params.router,
            tokenIn: params.token_in,
            tokenOut: params.token_out,
            stable: params.stable,
            amountIn: params.amount_in,
            minAmountOut: params.min_amount_out,
            recipient: params.recipient,
            deadline: params.deadline,
            pair: params.pair,
            minBaseReserve: U112::from(params.min_base_reserve),
            minTokenReserve: U112::from(params.min_token_reserve),
            maxBlockNumber: params.max_block_number,
        };
        call.abi_encode()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    #[test]
    fn encode_buy_v2_selector_and_args() {
        let params = BuyV2Params {
            router: address!("0x1111111111111111111111111111111111111111"),
            path: vec![
                address!("0x2222222222222222222222222222222222222222"),
                address!("0x3333333333333333333333333333333333333333"),
            ],
            amount_in: U256::from(10_000u64),
            min_amount_out: U256::from(9_500u64),
            recipient: address!("0x4444444444444444444444444444444444444444"),
            deadline: U256::from(999u64),
            pair: address!("0x5555555555555555555555555555555555555555"),
            min_base_reserve: 1000u128,
            min_token_reserve: 2000u128,
            max_block_number: 12_345,
        };

        let data = ExecutorTxBuilder::encode_buy_v2(params);
        assert_eq!(&data[0..4], &SonicSniperExecutor::buyV2Call::SELECTOR);

        let decoded = SonicSniperExecutor::buyV2Call::abi_decode(&data).unwrap();
        assert_eq!(
            decoded.router,
            address!("0x1111111111111111111111111111111111111111")
        );
        assert_eq!(
            decoded.path[0],
            address!("0x2222222222222222222222222222222222222222")
        );
        assert_eq!(
            decoded.path[1],
            address!("0x3333333333333333333333333333333333333333")
        );
        assert_eq!(decoded.amountIn, U256::from(10_000u64));
        assert_eq!(decoded.minAmountOut, U256::from(9_500u64));
        assert_eq!(
            decoded.recipient,
            address!("0x4444444444444444444444444444444444444444")
        );
        assert_eq!(decoded.deadline, U256::from(999u64));
        assert_eq!(
            decoded.pair,
            address!("0x5555555555555555555555555555555555555555")
        );
        assert_eq!(decoded.minBaseReserve, U112::from(1000u128));
        assert_eq!(decoded.minTokenReserve, U112::from(2000u128));
        assert_eq!(decoded.maxBlockNumber, 12_345u64);
    }

    #[test]
    fn encode_buy_solidly_selector_and_args() {
        let params = BuySolidlyParams {
            router: address!("0x1111111111111111111111111111111111111111"),
            token_in: address!("0x2222222222222222222222222222222222222222"),
            token_out: address!("0x3333333333333333333333333333333333333333"),
            stable: true,
            amount_in: U256::from(10_000u64),
            min_amount_out: U256::from(9_500u64),
            recipient: address!("0x4444444444444444444444444444444444444444"),
            deadline: U256::from(999u64),
            pair: address!("0x5555555555555555555555555555555555555555"),
            min_base_reserve: 1000u128,
            min_token_reserve: 2000u128,
            max_block_number: 12_345,
        };

        let data = ExecutorTxBuilder::encode_buy_solidly(params);
        assert_eq!(
            &data[0..4],
            &SonicSniperExecutor::buySolidlyCall::SELECTOR
        );

        let decoded = SonicSniperExecutor::buySolidlyCall::abi_decode(&data).unwrap();
        assert_eq!(
            decoded.router,
            address!("0x1111111111111111111111111111111111111111")
        );
        assert_eq!(
            decoded.tokenIn,
            address!("0x2222222222222222222222222222222222222222")
        );
        assert_eq!(
            decoded.tokenOut,
            address!("0x3333333333333333333333333333333333333333")
        );
        assert!(decoded.stable);
        assert_eq!(decoded.amountIn, U256::from(10_000u64));
        assert_eq!(decoded.minAmountOut, U256::from(9_500u64));
        assert_eq!(
            decoded.recipient,
            address!("0x4444444444444444444444444444444444444444")
        );
        assert_eq!(decoded.deadline, U256::from(999u64));
        assert_eq!(
            decoded.pair,
            address!("0x5555555555555555555555555555555555555555")
        );
        assert_eq!(decoded.minBaseReserve, U112::from(1000u128));
        assert_eq!(decoded.minTokenReserve, U112::from(2000u128));
        assert_eq!(decoded.maxBlockNumber, 12_345u64);
    }

    #[test]
    fn encode_buy_v2_eth_selector_and_args() {
        let params = BuyV2EthParams {
            router: address!("0x1111111111111111111111111111111111111111"),
            path: vec![
                address!("0x2222222222222222222222222222222222222222"),
                address!("0x3333333333333333333333333333333333333333"),
            ],
            amount_in: U256::from(10_000u64),
            min_amount_out: U256::from(9_500u64),
            recipient: address!("0x4444444444444444444444444444444444444444"),
            deadline: U256::from(999u64),
            pair: address!("0x5555555555555555555555555555555555555555"),
            min_base_reserve: 1000u128,
            min_token_reserve: 2000u128,
            max_block_number: 12_345,
        };

        let data = ExecutorTxBuilder::encode_buy_v2_eth(params);
        assert_eq!(&data[0..4], &SonicSniperExecutor::buyV2ETHCall::SELECTOR);

        let decoded = SonicSniperExecutor::buyV2ETHCall::abi_decode(&data).unwrap();
        assert_eq!(
            decoded.router,
            address!("0x1111111111111111111111111111111111111111")
        );
        assert_eq!(
            decoded.path[0],
            address!("0x2222222222222222222222222222222222222222")
        );
        assert_eq!(
            decoded.path[1],
            address!("0x3333333333333333333333333333333333333333")
        );
        assert_eq!(decoded.amountIn, U256::from(10_000u64));
        assert_eq!(decoded.minAmountOut, U256::from(9_500u64));
        assert_eq!(
            decoded.recipient,
            address!("0x4444444444444444444444444444444444444444")
        );
        assert_eq!(decoded.deadline, U256::from(999u64));
        assert_eq!(
            decoded.pair,
            address!("0x5555555555555555555555555555555555555555")
        );
        assert_eq!(decoded.minBaseReserve, U112::from(1000u128));
        assert_eq!(decoded.minTokenReserve, U112::from(2000u128));
        assert_eq!(decoded.maxBlockNumber, 12_345u64);
    }

    #[test]
    fn encode_buy_solidly_eth_selector_and_args() {
        let params = BuySolidlyEthParams {
            router: address!("0x1111111111111111111111111111111111111111"),
            token_in: address!("0x2222222222222222222222222222222222222222"),
            token_out: address!("0x3333333333333333333333333333333333333333"),
            stable: true,
            amount_in: U256::from(10_000u64),
            min_amount_out: U256::from(9_500u64),
            recipient: address!("0x4444444444444444444444444444444444444444"),
            deadline: U256::from(999u64),
            pair: address!("0x5555555555555555555555555555555555555555"),
            min_base_reserve: 1000u128,
            min_token_reserve: 2000u128,
            max_block_number: 12_345,
        };

        let data = ExecutorTxBuilder::encode_buy_solidly_eth(params);
        assert_eq!(
            &data[0..4],
            &SonicSniperExecutor::buySolidlyETHCall::SELECTOR
        );

        let decoded = SonicSniperExecutor::buySolidlyETHCall::abi_decode(&data).unwrap();
        assert_eq!(
            decoded.router,
            address!("0x1111111111111111111111111111111111111111")
        );
        assert_eq!(
            decoded.tokenIn,
            address!("0x2222222222222222222222222222222222222222")
        );
        assert_eq!(
            decoded.tokenOut,
            address!("0x3333333333333333333333333333333333333333")
        );
        assert!(decoded.stable);
        assert_eq!(decoded.amountIn, U256::from(10_000u64));
        assert_eq!(decoded.minAmountOut, U256::from(9_500u64));
        assert_eq!(
            decoded.recipient,
            address!("0x4444444444444444444444444444444444444444")
        );
        assert_eq!(decoded.deadline, U256::from(999u64));
        assert_eq!(
            decoded.pair,
            address!("0x5555555555555555555555555555555555555555")
        );
        assert_eq!(decoded.minBaseReserve, U112::from(1000u128));
        assert_eq!(decoded.minTokenReserve, U112::from(2000u128));
        assert_eq!(decoded.maxBlockNumber, 12_345u64);
    }

    #[test]
    fn encode_sell_v2_selector_and_args() {
        let params = SellV2Params {
            router: address!("0x1111111111111111111111111111111111111111"),
            path: vec![
                address!("0x2222222222222222222222222222222222222222"),
                address!("0x3333333333333333333333333333333333333333"),
            ],
            amount_in: U256::from(10_000u64),
            min_amount_out: U256::from(9_500u64),
            recipient: address!("0x4444444444444444444444444444444444444444"),
            deadline: U256::from(999u64),
            pair: address!("0x5555555555555555555555555555555555555555"),
            min_base_reserve: 1000u128,
            min_token_reserve: 2000u128,
            max_block_number: 12_345,
        };

        let data = ExecutorTxBuilder::encode_sell_v2(params);
        assert_eq!(&data[0..4], &SonicSniperExecutor::sellV2Call::SELECTOR);

        let decoded = SonicSniperExecutor::sellV2Call::abi_decode(&data).unwrap();
        assert_eq!(
            decoded.router,
            address!("0x1111111111111111111111111111111111111111")
        );
        assert_eq!(
            decoded.path[0],
            address!("0x2222222222222222222222222222222222222222")
        );
        assert_eq!(
            decoded.path[1],
            address!("0x3333333333333333333333333333333333333333")
        );
        assert_eq!(decoded.amountIn, U256::from(10_000u64));
        assert_eq!(decoded.minAmountOut, U256::from(9_500u64));
        assert_eq!(
            decoded.recipient,
            address!("0x4444444444444444444444444444444444444444")
        );
        assert_eq!(decoded.deadline, U256::from(999u64));
        assert_eq!(
            decoded.pair,
            address!("0x5555555555555555555555555555555555555555")
        );
        assert_eq!(decoded.minBaseReserve, U112::from(1000u128));
        assert_eq!(decoded.minTokenReserve, U112::from(2000u128));
        assert_eq!(decoded.maxBlockNumber, 12_345u64);
    }

    #[test]
    fn encode_sell_v2_eth_selector_and_args() {
        let params = SellV2EthParams {
            router: address!("0x1111111111111111111111111111111111111111"),
            path: vec![
                address!("0x2222222222222222222222222222222222222222"),
                address!("0x3333333333333333333333333333333333333333"),
            ],
            amount_in: U256::from(10_000u64),
            min_amount_out: U256::from(9_500u64),
            recipient: address!("0x4444444444444444444444444444444444444444"),
            deadline: U256::from(999u64),
            pair: address!("0x5555555555555555555555555555555555555555"),
            min_base_reserve: 1000u128,
            min_token_reserve: 2000u128,
            max_block_number: 12_345,
        };

        let data = ExecutorTxBuilder::encode_sell_v2_eth(params);
        assert_eq!(
            &data[0..4],
            &SonicSniperExecutor::sellV2ETHCall::SELECTOR
        );

        let decoded = SonicSniperExecutor::sellV2ETHCall::abi_decode(&data).unwrap();
        assert_eq!(
            decoded.router,
            address!("0x1111111111111111111111111111111111111111")
        );
        assert_eq!(
            decoded.path[0],
            address!("0x2222222222222222222222222222222222222222")
        );
        assert_eq!(
            decoded.path[1],
            address!("0x3333333333333333333333333333333333333333")
        );
        assert_eq!(decoded.amountIn, U256::from(10_000u64));
        assert_eq!(decoded.minAmountOut, U256::from(9_500u64));
        assert_eq!(
            decoded.recipient,
            address!("0x4444444444444444444444444444444444444444")
        );
        assert_eq!(decoded.deadline, U256::from(999u64));
        assert_eq!(
            decoded.pair,
            address!("0x5555555555555555555555555555555555555555")
        );
        assert_eq!(decoded.minBaseReserve, U112::from(1000u128));
        assert_eq!(decoded.minTokenReserve, U112::from(2000u128));
        assert_eq!(decoded.maxBlockNumber, 12_345u64);
    }

    #[test]
    fn encode_sell_solidly_selector_and_args() {
        let params = SellSolidlyParams {
            router: address!("0x1111111111111111111111111111111111111111"),
            token_in: address!("0x2222222222222222222222222222222222222222"),
            token_out: address!("0x3333333333333333333333333333333333333333"),
            stable: true,
            amount_in: U256::from(10_000u64),
            min_amount_out: U256::from(9_500u64),
            recipient: address!("0x4444444444444444444444444444444444444444"),
            deadline: U256::from(999u64),
            pair: address!("0x5555555555555555555555555555555555555555"),
            min_base_reserve: 1000u128,
            min_token_reserve: 2000u128,
            max_block_number: 12_345,
        };

        let data = ExecutorTxBuilder::encode_sell_solidly(params);
        assert_eq!(
            &data[0..4],
            &SonicSniperExecutor::sellSolidlyCall::SELECTOR
        );

        let decoded = SonicSniperExecutor::sellSolidlyCall::abi_decode(&data).unwrap();
        assert_eq!(
            decoded.router,
            address!("0x1111111111111111111111111111111111111111")
        );
        assert_eq!(
            decoded.tokenIn,
            address!("0x2222222222222222222222222222222222222222")
        );
        assert_eq!(
            decoded.tokenOut,
            address!("0x3333333333333333333333333333333333333333")
        );
        assert!(decoded.stable);
        assert_eq!(decoded.amountIn, U256::from(10_000u64));
        assert_eq!(decoded.minAmountOut, U256::from(9_500u64));
        assert_eq!(
            decoded.recipient,
            address!("0x4444444444444444444444444444444444444444")
        );
        assert_eq!(decoded.deadline, U256::from(999u64));
        assert_eq!(
            decoded.pair,
            address!("0x5555555555555555555555555555555555555555")
        );
        assert_eq!(decoded.minBaseReserve, U112::from(1000u128));
        assert_eq!(decoded.minTokenReserve, U112::from(2000u128));
        assert_eq!(decoded.maxBlockNumber, 12_345u64);
    }

    #[test]
    fn encode_sell_solidly_eth_selector_and_args() {
        let params = SellSolidlyEthParams {
            router: address!("0x1111111111111111111111111111111111111111"),
            token_in: address!("0x2222222222222222222222222222222222222222"),
            token_out: address!("0x3333333333333333333333333333333333333333"),
            stable: true,
            amount_in: U256::from(10_000u64),
            min_amount_out: U256::from(9_500u64),
            recipient: address!("0x4444444444444444444444444444444444444444"),
            deadline: U256::from(999u64),
            pair: address!("0x5555555555555555555555555555555555555555"),
            min_base_reserve: 1000u128,
            min_token_reserve: 2000u128,
            max_block_number: 12_345,
        };

        let data = ExecutorTxBuilder::encode_sell_solidly_eth(params);
        assert_eq!(
            &data[0..4],
            &SonicSniperExecutor::sellSolidlyETHCall::SELECTOR
        );

        let decoded = SonicSniperExecutor::sellSolidlyETHCall::abi_decode(&data).unwrap();
        assert_eq!(
            decoded.router,
            address!("0x1111111111111111111111111111111111111111")
        );
        assert_eq!(
            decoded.tokenIn,
            address!("0x2222222222222222222222222222222222222222")
        );
        assert_eq!(
            decoded.tokenOut,
            address!("0x3333333333333333333333333333333333333333")
        );
        assert!(decoded.stable);
        assert_eq!(decoded.amountIn, U256::from(10_000u64));
        assert_eq!(decoded.minAmountOut, U256::from(9_500u64));
        assert_eq!(
            decoded.recipient,
            address!("0x4444444444444444444444444444444444444444")
        );
        assert_eq!(decoded.deadline, U256::from(999u64));
        assert_eq!(
            decoded.pair,
            address!("0x5555555555555555555555555555555555555555")
        );
        assert_eq!(decoded.minBaseReserve, U112::from(1000u128));
        assert_eq!(decoded.minTokenReserve, U112::from(2000u128));
        assert_eq!(decoded.maxBlockNumber, 12_345u64);
    }
}
