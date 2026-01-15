use alloy::sol;

sol! {
    interface SonicSniperExecutor {
        function buyV2(
            address router,
            address[] calldata path,
            uint256 amountIn,
            uint256 minAmountOut,
            address recipient,
            uint256 deadline,
            address pair,
            uint112 minBaseReserve,
            uint112 minTokenReserve,
            uint64 maxBlockNumber
        ) external returns (uint256 amountOut);

        function buySolidly(
            address router,
            address tokenIn,
            address tokenOut,
            bool stable,
            uint256 amountIn,
            uint256 minAmountOut,
            address recipient,
            uint256 deadline,
            address pair,
            uint112 minBaseReserve,
            uint112 minTokenReserve,
            uint64 maxBlockNumber
        ) external returns (uint256 amountOut);

        function rescueToken(address token, address to, uint256 amount) external;
        function rescueETH(address to, uint256 amount) external;
    }
}
