// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

interface IERC20 {
    function totalSupply() external view returns (uint256);
    function balanceOf(address account) external view returns (uint256);
    function transfer(address to, uint256 value) external returns (bool);
    function allowance(address owner, address spender) external view returns (uint256);
    function approve(address spender, uint256 value) external returns (bool);
    function transferFrom(address from, address to, uint256 value) external returns (bool);
}

library SafeERC20 {
    function safeTransfer(IERC20 token, address to, uint256 value) internal {
        _callOptionalReturn(token, abi.encodeWithSelector(token.transfer.selector, to, value));
    }

    function safeTransferFrom(IERC20 token, address from, address to, uint256 value) internal {
        _callOptionalReturn(token, abi.encodeWithSelector(token.transferFrom.selector, from, to, value));
    }

    function safeApprove(IERC20 token, address spender, uint256 value) internal {
        _callOptionalReturn(token, abi.encodeWithSelector(token.approve.selector, spender, value));
    }

    function _callOptionalReturn(IERC20 token, bytes memory data) private {
        (bool success, bytes memory returndata) = address(token).call(data);
        require(success, "SafeERC20: call failed");
        if (returndata.length > 0) {
            require(abi.decode(returndata, (bool)), "SafeERC20: operation failed");
        }
    }
}

interface IUniswapV2Router02 {
    function swapExactTokensForTokens(
        uint256 amountIn,
        uint256 amountOutMin,
        address[] calldata path,
        address to,
        uint256 deadline
    ) external returns (uint256[] memory amounts);

    function swapExactTokensForTokensSupportingFeeOnTransferTokens(
        uint256 amountIn,
        uint256 amountOutMin,
        address[] calldata path,
        address to,
        uint256 deadline
    ) external;

    function swapExactETHForTokens(
        uint256 amountOutMin,
        address[] calldata path,
        address to,
        uint256 deadline
    ) external payable returns (uint256[] memory amounts);

    function swapExactETHForTokensSupportingFeeOnTransferTokens(
        uint256 amountOutMin,
        address[] calldata path,
        address to,
        uint256 deadline
    ) external payable;
}

interface ISolidlyRouter {
    struct Route {
        address from;
        address to;
        bool stable;
    }

    function swapExactTokensForTokens(
        uint256 amountIn,
        uint256 amountOutMin,
        Route[] calldata routes,
        address to,
        uint256 deadline
    ) external returns (uint256[] memory amounts);

    function swapExactETHForTokens(
        uint256 amountOutMin,
        Route[] calldata routes,
        address to,
        uint256 deadline
    ) external payable returns (uint256[] memory amounts);
}

interface IUniswapV2Pair {
    function getReserves() external view returns (uint112 reserve0, uint112 reserve1, uint32 blockTimestampLast);
}

contract SonicSniperExecutor {
    using SafeERC20 for IERC20;

    address public owner;
    bool public useFeeOnTransfer;

    event Bought(
        address indexed router,
        address indexed tokenIn,
        address indexed tokenOut,
        uint256 amountIn,
        uint256 minOut,
        address recipient,
        uint256 blockNumber
    );

    modifier onlyOwner() {
        require(msg.sender == owner, "NOT_OWNER");
        _;
    }

    constructor() {
        owner = msg.sender;
    }

    function setUseFeeOnTransfer(bool value) external onlyOwner {
        useFeeOnTransfer = value;
    }

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
    ) external onlyOwner returns (uint256 amountOut) {
        require(path.length >= 2, "BAD_PATH");

        if (maxBlockNumber != 0) {
            require(block.number <= maxBlockNumber, "BLOCK_GUARD");
        }

        if (pair != address(0)) {
            (uint112 r0, uint112 r1,) = IUniswapV2Pair(pair).getReserves();
            require(r0 >= minBaseReserve && r1 >= minTokenReserve, "RESERVE_GUARD");
        }

        address to = recipient == address(0) ? owner : recipient;
        IERC20 tokenIn = IERC20(path[0]);

        uint256 balance = tokenIn.balanceOf(address(this));
        if (balance < amountIn) {
            tokenIn.safeTransferFrom(owner, address(this), amountIn - balance);
        }

        tokenIn.safeApprove(router, 0);
        tokenIn.safeApprove(router, amountIn);

        if (useFeeOnTransfer) {
            uint256 beforeBal = IERC20(path[path.length - 1]).balanceOf(to);
            IUniswapV2Router02(router).swapExactTokensForTokensSupportingFeeOnTransferTokens(
                amountIn,
                minAmountOut,
                path,
                to,
                deadline
            );
            uint256 afterBal = IERC20(path[path.length - 1]).balanceOf(to);
            amountOut = afterBal - beforeBal;
        } else {
            uint256[] memory amounts = IUniswapV2Router02(router).swapExactTokensForTokens(
                amountIn,
                minAmountOut,
                path,
                to,
                deadline
            );
            amountOut = amounts[amounts.length - 1];
        }

        tokenIn.safeApprove(router, 0);

        emit Bought(router, path[0], path[path.length - 1], amountIn, minAmountOut, to, block.number);
    }

    function buyV2ETH(
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
    ) external payable onlyOwner returns (uint256 amountOut) {
        require(path.length >= 2, "BAD_PATH");
        require(msg.value == amountIn, "BAD_VALUE");

        if (maxBlockNumber != 0) {
            require(block.number <= maxBlockNumber, "BLOCK_GUARD");
        }

        if (pair != address(0)) {
            (uint112 r0, uint112 r1,) = IUniswapV2Pair(pair).getReserves();
            require(r0 >= minBaseReserve && r1 >= minTokenReserve, "RESERVE_GUARD");
        }

        address to = recipient == address(0) ? owner : recipient;

        if (useFeeOnTransfer) {
            uint256 beforeBal = IERC20(path[path.length - 1]).balanceOf(to);
            IUniswapV2Router02(router).swapExactETHForTokensSupportingFeeOnTransferTokens{value: amountIn}(
                minAmountOut,
                path,
                to,
                deadline
            );
            uint256 afterBal = IERC20(path[path.length - 1]).balanceOf(to);
            amountOut = afterBal - beforeBal;
        } else {
            uint256[] memory amounts =
                IUniswapV2Router02(router).swapExactETHForTokens{value: amountIn}(minAmountOut, path, to, deadline);
            amountOut = amounts[amounts.length - 1];
        }

        emit Bought(router, address(0), path[path.length - 1], amountIn, minAmountOut, to, block.number);
    }

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
    ) external onlyOwner returns (uint256 amountOut) {
        if (maxBlockNumber != 0) {
            require(block.number <= maxBlockNumber, "BLOCK_GUARD");
        }

        if (pair != address(0)) {
            (uint112 r0, uint112 r1,) = IUniswapV2Pair(pair).getReserves();
            require(r0 >= minBaseReserve && r1 >= minTokenReserve, "RESERVE_GUARD");
        }

        address to = recipient == address(0) ? owner : recipient;
        IERC20 tokenInErc20 = IERC20(tokenIn);

        uint256 balance = tokenInErc20.balanceOf(address(this));
        if (balance < amountIn) {
            tokenInErc20.safeTransferFrom(owner, address(this), amountIn - balance);
        }

        tokenInErc20.safeApprove(router, 0);
        tokenInErc20.safeApprove(router, amountIn);

        ISolidlyRouter.Route[] memory routes = new ISolidlyRouter.Route[](1);
        routes[0] = ISolidlyRouter.Route({from: tokenIn, to: tokenOut, stable: stable});

        uint256[] memory amounts =
            ISolidlyRouter(router).swapExactTokensForTokens(amountIn, minAmountOut, routes, to, deadline);
        amountOut = amounts[amounts.length - 1];

        tokenInErc20.safeApprove(router, 0);

        emit Bought(router, tokenIn, tokenOut, amountIn, minAmountOut, to, block.number);
    }

    function buySolidlyETH(
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
    ) external payable onlyOwner returns (uint256 amountOut) {
        require(msg.value == amountIn, "BAD_VALUE");

        if (maxBlockNumber != 0) {
            require(block.number <= maxBlockNumber, "BLOCK_GUARD");
        }

        if (pair != address(0)) {
            (uint112 r0, uint112 r1,) = IUniswapV2Pair(pair).getReserves();
            require(r0 >= minBaseReserve && r1 >= minTokenReserve, "RESERVE_GUARD");
        }

        address to = recipient == address(0) ? owner : recipient;

        ISolidlyRouter.Route[] memory routes = new ISolidlyRouter.Route[](1);
        routes[0] = ISolidlyRouter.Route({from: tokenIn, to: tokenOut, stable: stable});

        uint256[] memory amounts =
            ISolidlyRouter(router).swapExactETHForTokens{value: amountIn}(minAmountOut, routes, to, deadline);
        amountOut = amounts[amounts.length - 1];

        emit Bought(router, address(0), tokenOut, amountIn, minAmountOut, to, block.number);
    }

    function rescueToken(address token, address to, uint256 amount) external onlyOwner {
        IERC20(token).safeTransfer(to, amount);
    }

    function rescueETH(address to, uint256 amount) external onlyOwner {
        (bool ok,) = to.call{value: amount}("");
        require(ok, "RESCUE_ETH_FAILED");
    }

    receive() external payable {}
}
