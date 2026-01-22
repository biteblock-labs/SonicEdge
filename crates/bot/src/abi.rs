use alloy::sol;

sol! {
    interface IERC20 {
        function balanceOf(address account) external view returns (uint256);
    }
}
