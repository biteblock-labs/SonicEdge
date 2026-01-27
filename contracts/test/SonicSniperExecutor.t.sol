// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "../SonicSniperExecutor.sol";

interface Vm {
    function deal(address who, uint256 newBalance) external;
    function expectRevert(bytes calldata) external;
    function prank(address newSender) external;
    function roll(uint256 newHeight) external;
}

contract TestBase {
    Vm internal constant vm = Vm(address(uint160(uint256(keccak256("hevm cheat code")))));

    function assertTrue(bool value, string memory message) internal pure {
        require(value, message);
    }

    function assertEq(uint256 a, uint256 b, string memory message) internal pure {
        require(a == b, message);
    }

    function assertEq(address a, address b, string memory message) internal pure {
        require(a == b, message);
    }

    function assertEq(string memory a, string memory b, string memory message) internal pure {
        require(keccak256(bytes(a)) == keccak256(bytes(b)), message);
    }
}

interface IMintableERC20 {
    function mint(address to, uint256 amount) external;
}

contract MockERC20 is IERC20 {
    string public name;
    string public symbol;
    uint8 public decimals;

    uint256 public totalSupply;
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    constructor(string memory name_, string memory symbol_, uint8 decimals_) {
        name = name_;
        symbol = symbol_;
        decimals = decimals_;
    }

    function mint(address to, uint256 amount) external {
        totalSupply += amount;
        balanceOf[to] += amount;
    }

    function transfer(address to, uint256 value) external returns (bool) {
        _transfer(msg.sender, to, value);
        return true;
    }

    function approve(address spender, uint256 value) external returns (bool) {
        allowance[msg.sender][spender] = value;
        return true;
    }

    function transferFrom(address from, address to, uint256 value) external returns (bool) {
        uint256 allowed = allowance[from][msg.sender];
        require(allowed >= value, "ALLOWANCE");
        if (allowed != type(uint256).max) {
            allowance[from][msg.sender] = allowed - value;
        }
        _transfer(from, to, value);
        return true;
    }

    function _transfer(address from, address to, uint256 value) internal virtual {
        require(to != address(0), "ZERO");
        uint256 bal = balanceOf[from];
        require(bal >= value, "BAL");
        unchecked {
            balanceOf[from] = bal - value;
            balanceOf[to] += value;
        }
    }
}

contract FeeOnTransferERC20 is MockERC20 {
    uint256 public feeBps;

    constructor(string memory name_, string memory symbol_, uint8 decimals_, uint256 feeBps_)
        MockERC20(name_, symbol_, decimals_)
    {
        feeBps = feeBps_;
    }

    function _transfer(address from, address to, uint256 value) internal override {
        require(to != address(0), "ZERO");
        uint256 bal = balanceOf[from];
        require(bal >= value, "BAL");
        uint256 fee = (value * feeBps) / 10_000;
        uint256 received = value - fee;
        unchecked {
            balanceOf[from] = bal - value;
            balanceOf[to] += received;
            totalSupply -= fee;
        }
    }
}

contract MockWETH is MockERC20, IWETH {
    constructor() MockERC20("Wrapped S", "wS", 18) {}

    function withdraw(uint256 amount) external {
        uint256 bal = balanceOf[msg.sender];
        require(bal >= amount, "BAL");
        balanceOf[msg.sender] = bal - amount;
        totalSupply -= amount;
        (bool ok,) = msg.sender.call{value: amount}("");
        require(ok, "ETH_TRANSFER_FAILED");
    }

    receive() external payable {}
}

contract MockPair is IUniswapV2Pair {
    uint112 private reserve0;
    uint112 private reserve1;

    function setReserves(uint112 r0, uint112 r1) external {
        reserve0 = r0;
        reserve1 = r1;
    }

    function getReserves() external view returns (uint112 r0, uint112 r1, uint32) {
        return (reserve0, reserve1, 0);
    }
}

contract MockV2Router is IUniswapV2Router02 {
    mapping(bytes32 => uint256) public rates;

    function setRate(address tokenIn, address tokenOut, uint256 rate) external {
        rates[keccak256(abi.encodePacked(tokenIn, tokenOut))] = rate;
    }

    function _rate(address tokenIn, address tokenOut) internal view returns (uint256) {
        uint256 rate = rates[keccak256(abi.encodePacked(tokenIn, tokenOut))];
        require(rate > 0, "RATE");
        return rate;
    }

    function swapExactTokensForTokens(
        uint256 amountIn,
        uint256,
        address[] calldata path,
        address to,
        uint256
    ) external returns (uint256[] memory amounts) {
        IERC20(path[0]).transferFrom(msg.sender, address(this), amountIn);
        uint256 amountOut = (amountIn * _rate(path[0], path[path.length - 1])) / 1e18;
        IMintableERC20(path[path.length - 1]).mint(to, amountOut);
        amounts = new uint256[](path.length);
        amounts[0] = amountIn;
        amounts[path.length - 1] = amountOut;
    }

    function swapExactTokensForTokensSupportingFeeOnTransferTokens(
        uint256 amountIn,
        uint256,
        address[] calldata path,
        address to,
        uint256
    ) external {
        IERC20 tokenIn = IERC20(path[0]);
        uint256 beforeBal = tokenIn.balanceOf(address(this));
        tokenIn.transferFrom(msg.sender, address(this), amountIn);
        uint256 received = tokenIn.balanceOf(address(this)) - beforeBal;
        uint256 amountOut = (received * _rate(path[0], path[path.length - 1])) / 1e18;
        IMintableERC20(path[path.length - 1]).mint(to, amountOut);
    }

    function swapExactTokensForETH(
        uint256 amountIn,
        uint256,
        address[] calldata path,
        address to,
        uint256
    ) external returns (uint256[] memory amounts) {
        IERC20(path[0]).transferFrom(msg.sender, address(this), amountIn);
        uint256 amountOut = (amountIn * _rate(path[0], address(0))) / 1e18;
        (bool ok,) = to.call{value: amountOut}("");
        require(ok, "ETH_TRANSFER_FAILED");
        amounts = new uint256[](path.length);
        amounts[0] = amountIn;
        amounts[path.length - 1] = amountOut;
    }

    function swapExactTokensForETHSupportingFeeOnTransferTokens(
        uint256 amountIn,
        uint256,
        address[] calldata path,
        address to,
        uint256
    ) external {
        IERC20 tokenIn = IERC20(path[0]);
        uint256 beforeBal = tokenIn.balanceOf(address(this));
        tokenIn.transferFrom(msg.sender, address(this), amountIn);
        uint256 received = tokenIn.balanceOf(address(this)) - beforeBal;
        uint256 amountOut = (received * _rate(path[0], address(0))) / 1e18;
        (bool ok,) = to.call{value: amountOut}("");
        require(ok, "ETH_TRANSFER_FAILED");
    }

    function swapExactETHForTokens(
        uint256,
        address[] calldata path,
        address to,
        uint256
    ) external payable returns (uint256[] memory amounts) {
        uint256 amountOut = (msg.value * _rate(address(0), path[path.length - 1])) / 1e18;
        IMintableERC20(path[path.length - 1]).mint(to, amountOut);
        amounts = new uint256[](path.length);
        amounts[0] = msg.value;
        amounts[path.length - 1] = amountOut;
    }

    function swapExactETHForTokensSupportingFeeOnTransferTokens(
        uint256,
        address[] calldata path,
        address to,
        uint256
    ) external payable {
        uint256 amountOut = (msg.value * _rate(address(0), path[path.length - 1])) / 1e18;
        IMintableERC20(path[path.length - 1]).mint(to, amountOut);
    }

    receive() external payable {}
}

contract MockSolidlyRouter is ISolidlyRouter {
    mapping(bytes32 => uint256) public rates;

    function setRate(address tokenIn, address tokenOut, bool stable, uint256 rate) external {
        rates[keccak256(abi.encodePacked(tokenIn, tokenOut, stable))] = rate;
    }

    function _rate(address tokenIn, address tokenOut, bool stable) internal view returns (uint256) {
        uint256 rate = rates[keccak256(abi.encodePacked(tokenIn, tokenOut, stable))];
        require(rate > 0, "RATE");
        return rate;
    }

    function swapExactTokensForTokens(
        uint256 amountIn,
        uint256,
        Route[] calldata routes,
        address to,
        uint256
    ) external returns (uint256[] memory amounts) {
        require(routes.length == 1, "ROUTE_LEN");
        IERC20(routes[0].from).transferFrom(msg.sender, address(this), amountIn);
        uint256 amountOut = (amountIn * _rate(routes[0].from, routes[0].to, routes[0].stable)) / 1e18;
        IMintableERC20(routes[0].to).mint(to, amountOut);
        amounts = new uint256[](2);
        amounts[0] = amountIn;
        amounts[1] = amountOut;
    }

    function swapExactETHForTokens(
        uint256,
        Route[] calldata routes,
        address to,
        uint256
    ) external payable returns (uint256[] memory amounts) {
        require(routes.length == 1, "ROUTE_LEN");
        uint256 amountOut = (msg.value * _rate(routes[0].from, routes[0].to, routes[0].stable)) / 1e18;
        IMintableERC20(routes[0].to).mint(to, amountOut);
        amounts = new uint256[](2);
        amounts[0] = msg.value;
        amounts[1] = amountOut;
    }

    receive() external payable {}
}

contract SonicSniperExecutorTest is TestBase {
    SonicSniperExecutor private executor;
    MockERC20 private tokenA;
    MockERC20 private tokenB;
    MockERC20 private baseToken;
    MockWETH private wS;
    FeeOnTransferERC20 private fotToken;
    MockV2Router private v2Router;
    MockSolidlyRouter private solidlyRouter;
    MockPair private pair;

    address private recipient = address(0xBEEF);

    function setUp() public {
        executor = new SonicSniperExecutor();
        tokenA = new MockERC20("TokenA", "TKA", 18);
        tokenB = new MockERC20("TokenB", "TKB", 18);
        baseToken = new MockERC20("Base", "BASE", 18);
        wS = new MockWETH();
        fotToken = new FeeOnTransferERC20("FeeToken", "FEE", 18, 1000);
        v2Router = new MockV2Router();
        solidlyRouter = new MockSolidlyRouter();
        pair = new MockPair();

        tokenA.mint(address(this), 1_000_000e18);
        tokenB.mint(address(this), 1_000_000e18);
        baseToken.mint(address(this), 1_000_000e18);
        fotToken.mint(address(this), 1_000_000e18);

        tokenA.approve(address(executor), type(uint256).max);
        tokenB.approve(address(executor), type(uint256).max);
        baseToken.approve(address(executor), type(uint256).max);
        fotToken.approve(address(executor), type(uint256).max);

        v2Router.setRate(address(tokenA), address(tokenB), 2e18);
        v2Router.setRate(address(tokenB), address(tokenA), 5e17);
        v2Router.setRate(address(tokenA), address(0), 1e18);
        v2Router.setRate(address(0), address(tokenA), 3e18);

        solidlyRouter.setRate(address(tokenA), address(tokenB), false, 2e18);
        solidlyRouter.setRate(address(tokenB), address(tokenA), false, 5e17);
        solidlyRouter.setRate(address(wS), address(tokenA), false, 4e18);
        solidlyRouter.setRate(address(tokenA), address(wS), false, 1e18);

        pair.setReserves(1_000_000, 1_000_000);
    }

    function test_buyV2_basic() public {
        uint256 amountIn = 1e18;
        address[] memory path = new address[](2);
        path[0] = address(tokenA);
        path[1] = address(tokenB);
        uint256 beforeOut = tokenB.balanceOf(recipient);

        executor.buyV2(
            address(v2Router),
            path,
            amountIn,
            0,
            recipient,
            block.timestamp + 1,
            address(pair),
            1,
            1,
            0
        );

        uint256 afterOut = tokenB.balanceOf(recipient);
        assertEq(afterOut - beforeOut, 2e18, "buyV2 output");
    }

    function test_buyV2_bad_path_reverts() public {
        address[] memory path = new address[](1);
        path[0] = address(tokenA);
        vm.expectRevert(bytes("BAD_PATH"));
        executor.buyV2(
            address(v2Router),
            path,
            1e18,
            0,
            recipient,
            block.timestamp + 1,
            address(pair),
            1,
            1,
            0
        );
    }

    function test_buyV2_eth_bad_path_reverts() public {
        address[] memory path = new address[](1);
        path[0] = address(tokenA);
        vm.expectRevert(bytes("BAD_PATH"));
        executor.buyV2ETH{value: 1e18}(
            address(v2Router),
            path,
            1e18,
            0,
            recipient,
            block.timestamp + 1,
            address(pair),
            1,
            1,
            0
        );
    }

    function test_buyV2_block_guard() public {
        address[] memory path = new address[](2);
        path[0] = address(tokenA);
        path[1] = address(tokenB);
        vm.roll(2);
        vm.expectRevert(bytes("BLOCK_GUARD"));
        executor.buyV2(
            address(v2Router),
            path,
            1e18,
            0,
            recipient,
            block.timestamp + 1,
            address(pair),
            1,
            1,
            1
        );
    }

    function test_buyV2_eth_block_guard() public {
        address[] memory path = new address[](2);
        path[0] = address(0);
        path[1] = address(tokenA);
        vm.roll(2);
        vm.expectRevert(bytes("BLOCK_GUARD"));
        executor.buyV2ETH{value: 1e18}(
            address(v2Router),
            path,
            1e18,
            0,
            recipient,
            block.timestamp + 1,
            address(pair),
            1,
            1,
            1
        );
    }

    function test_buyV2_reserve_guard() public {
        address[] memory path = new address[](2);
        path[0] = address(tokenA);
        path[1] = address(tokenB);
        pair.setReserves(0, 0);
        vm.expectRevert(bytes("RESERVE_GUARD"));
        executor.buyV2(
            address(v2Router),
            path,
            1e18,
            0,
            recipient,
            block.timestamp + 1,
            address(pair),
            10,
            10,
            0
        );
    }

    function test_buyV2_allowance_cleared() public {
        address[] memory path = new address[](2);
        path[0] = address(tokenA);
        path[1] = address(tokenB);

        executor.buyV2(
            address(v2Router),
            path,
            1e18,
            0,
            recipient,
            block.timestamp + 1,
            address(pair),
            1,
            1,
            0
        );

        uint256 allowance = tokenA.allowance(address(executor), address(v2Router));
        assertEq(allowance, 0, "buyV2 allowance cleared");
    }

    function test_buyV2_fee_on_transfer() public {
        executor.setUseFeeOnTransfer(true);
        uint256 amountIn = 1e18;
        address[] memory path = new address[](2);
        path[0] = address(fotToken);
        path[1] = address(tokenB);
        v2Router.setRate(address(fotToken), address(tokenB), 1e18);
        uint256 beforeOut = tokenB.balanceOf(recipient);

        executor.buyV2(
            address(v2Router),
            path,
            amountIn,
            0,
            recipient,
            block.timestamp + 1,
            address(pair),
            1,
            1,
            0
        );

        uint256 afterOut = tokenB.balanceOf(recipient);
        uint256 expectedOut = (amountIn * 9000) / 10_000;
        expectedOut = (expectedOut * 9000) / 10_000;
        assertEq(afterOut - beforeOut, expectedOut, "fee on transfer output");
    }

    function test_buyV2_eth() public {
        uint256 amountIn = 1e18;
        address[] memory path = new address[](2);
        path[0] = address(0);
        path[1] = address(tokenA);
        uint256 beforeOut = tokenA.balanceOf(recipient);

        executor.buyV2ETH{value: amountIn}(
            address(v2Router),
            path,
            amountIn,
            0,
            recipient,
            block.timestamp + 1,
            address(pair),
            1,
            1,
            0
        );

        uint256 afterOut = tokenA.balanceOf(recipient);
        assertEq(afterOut - beforeOut, 3e18, "buyV2ETH output");
    }

    function test_buyV2_uses_existing_balance() public {
        address[] memory path = new address[](2);
        path[0] = address(tokenA);
        path[1] = address(tokenB);

        uint256 prebalance = 5e17;
        tokenA.mint(address(executor), prebalance);
        uint256 ownerBefore = tokenA.balanceOf(address(this));

        executor.buyV2(
            address(v2Router),
            path,
            1e18,
            0,
            recipient,
            block.timestamp + 1,
            address(pair),
            1,
            1,
            0
        );

        uint256 ownerAfter = tokenA.balanceOf(address(this));
        assertEq(ownerBefore - ownerAfter, 5e17, "owner delta uses executor balance");
    }

    function test_buyV2_eth_bad_value_reverts() public {
        address[] memory path = new address[](2);
        path[0] = address(0);
        path[1] = address(tokenA);
        vm.expectRevert(bytes("BAD_VALUE"));
        executor.buyV2ETH{value: 0}(
            address(v2Router),
            path,
            1e18,
            0,
            recipient,
            block.timestamp + 1,
            address(pair),
            1,
            1,
            0
        );
    }

    function test_sellV2_basic() public {
        uint256 amountIn = 1e18;
        address[] memory path = new address[](2);
        path[0] = address(tokenB);
        path[1] = address(tokenA);
        uint256 beforeOut = tokenA.balanceOf(recipient);

        executor.sellV2(
            address(v2Router),
            path,
            amountIn,
            0,
            recipient,
            block.timestamp + 1,
            address(pair),
            1,
            1,
            0
        );

        uint256 afterOut = tokenA.balanceOf(recipient);
        assertEq(afterOut - beforeOut, 5e17, "sellV2 output");
    }

    function test_sellV2_bad_path_reverts() public {
        address[] memory path = new address[](1);
        path[0] = address(tokenB);
        vm.expectRevert(bytes("BAD_PATH"));
        executor.sellV2(
            address(v2Router),
            path,
            1e18,
            0,
            recipient,
            block.timestamp + 1,
            address(pair),
            1,
            1,
            0
        );
    }

    function test_sellV2_block_guard() public {
        address[] memory path = new address[](2);
        path[0] = address(tokenB);
        path[1] = address(tokenA);
        vm.roll(2);
        vm.expectRevert(bytes("BLOCK_GUARD"));
        executor.sellV2(
            address(v2Router),
            path,
            1e18,
            0,
            recipient,
            block.timestamp + 1,
            address(pair),
            1,
            1,
            1
        );
    }

    function test_sellV2_reserve_guard() public {
        address[] memory path = new address[](2);
        path[0] = address(tokenB);
        path[1] = address(tokenA);
        pair.setReserves(0, 0);
        vm.expectRevert(bytes("RESERVE_GUARD"));
        executor.sellV2(
            address(v2Router),
            path,
            1e18,
            0,
            recipient,
            block.timestamp + 1,
            address(pair),
            10,
            10,
            0
        );
    }

    function test_sellV2_allowance_cleared() public {
        address[] memory path = new address[](2);
        path[0] = address(tokenB);
        path[1] = address(tokenA);

        executor.sellV2(
            address(v2Router),
            path,
            1e18,
            0,
            recipient,
            block.timestamp + 1,
            address(pair),
            1,
            1,
            0
        );

        uint256 allowance = tokenB.allowance(address(executor), address(v2Router));
        assertEq(allowance, 0, "sellV2 allowance cleared");
    }

    function test_sellV2_eth() public {
        uint256 amountIn = 1e18;
        address[] memory path = new address[](2);
        path[0] = address(tokenA);
        path[1] = address(0);
        vm.deal(address(v2Router), 10 ether);
        uint256 beforeOut = recipient.balance;

        executor.sellV2ETH(
            address(v2Router),
            path,
            amountIn,
            0,
            recipient,
            block.timestamp + 1,
            address(pair),
            1,
            1,
            0
        );

        uint256 afterOut = recipient.balance;
        assertEq(afterOut - beforeOut, 1e18, "sellV2ETH output");
    }

    function test_buy_solidly_eth_bad_value_reverts() public {
        vm.expectRevert(bytes("BAD_VALUE"));
        executor.buySolidlyETH{value: 0}(
            address(solidlyRouter),
            address(wS),
            address(tokenA),
            false,
            1e18,
            0,
            recipient,
            block.timestamp + 1,
            address(pair),
            1,
            1,
            0
        );
    }

    function test_buy_solidly_block_guard() public {
        vm.roll(2);
        vm.expectRevert(bytes("BLOCK_GUARD"));
        executor.buySolidly(
            address(solidlyRouter),
            address(tokenA),
            address(tokenB),
            false,
            1e18,
            0,
            recipient,
            block.timestamp + 1,
            address(pair),
            1,
            1,
            1
        );
    }

    function test_buy_solidly_reserve_guard() public {
        pair.setReserves(0, 0);
        vm.expectRevert(bytes("RESERVE_GUARD"));
        executor.buySolidly(
            address(solidlyRouter),
            address(tokenA),
            address(tokenB),
            false,
            1e18,
            0,
            recipient,
            block.timestamp + 1,
            address(pair),
            10,
            10,
            0
        );
    }

    function test_buy_solidly_allowance_cleared() public {
        executor.buySolidly(
            address(solidlyRouter),
            address(tokenA),
            address(tokenB),
            false,
            1e18,
            0,
            recipient,
            block.timestamp + 1,
            address(pair),
            1,
            1,
            0
        );
        uint256 allowance = tokenA.allowance(address(executor), address(solidlyRouter));
        assertEq(allowance, 0, "buySolidly allowance cleared");
    }

    function test_buy_solidly_stable_route() public {
        solidlyRouter.setRate(address(tokenA), address(tokenB), true, 3e18);
        uint256 beforeOut = tokenB.balanceOf(recipient);

        executor.buySolidly(
            address(solidlyRouter),
            address(tokenA),
            address(tokenB),
            true,
            1e18,
            0,
            recipient,
            block.timestamp + 1,
            address(pair),
            1,
            1,
            0
        );

        uint256 afterOut = tokenB.balanceOf(recipient);
        assertEq(afterOut - beforeOut, 3e18, "stable route output");
    }

    function test_buy_solidly_basic() public {
        uint256 amountIn = 1e18;
        uint256 beforeOut = tokenB.balanceOf(recipient);

        executor.buySolidly(
            address(solidlyRouter),
            address(tokenA),
            address(tokenB),
            false,
            amountIn,
            0,
            recipient,
            block.timestamp + 1,
            address(pair),
            1,
            1,
            0
        );

        uint256 afterOut = tokenB.balanceOf(recipient);
        assertEq(afterOut - beforeOut, 2e18, "buySolidly output");
    }

    function test_sell_solidly_block_guard() public {
        vm.roll(2);
        vm.expectRevert(bytes("BLOCK_GUARD"));
        executor.sellSolidly(
            address(solidlyRouter),
            address(tokenB),
            address(tokenA),
            false,
            1e18,
            0,
            recipient,
            block.timestamp + 1,
            address(pair),
            1,
            1,
            1
        );
    }

    function test_sell_solidly_reserve_guard() public {
        pair.setReserves(0, 0);
        vm.expectRevert(bytes("RESERVE_GUARD"));
        executor.sellSolidly(
            address(solidlyRouter),
            address(tokenB),
            address(tokenA),
            false,
            1e18,
            0,
            recipient,
            block.timestamp + 1,
            address(pair),
            10,
            10,
            0
        );
    }

    function test_sell_solidly_allowance_cleared() public {
        executor.sellSolidly(
            address(solidlyRouter),
            address(tokenB),
            address(tokenA),
            false,
            1e18,
            0,
            recipient,
            block.timestamp + 1,
            address(pair),
            1,
            1,
            0
        );
        uint256 allowance = tokenB.allowance(address(executor), address(solidlyRouter));
        assertEq(allowance, 0, "sellSolidly allowance cleared");
    }

    function test_sell_solidly_basic() public {
        uint256 amountIn = 1e18;
        uint256 beforeOut = tokenA.balanceOf(recipient);

        executor.sellSolidly(
            address(solidlyRouter),
            address(tokenB),
            address(tokenA),
            false,
            amountIn,
            0,
            recipient,
            block.timestamp + 1,
            address(pair),
            1,
            1,
            0
        );

        uint256 afterOut = tokenA.balanceOf(recipient);
        assertEq(afterOut - beforeOut, 5e17, "sellSolidly output");
    }

    function test_buy_solidly_eth() public {
        uint256 amountIn = 1e18;
        vm.deal(address(this), 10 ether);
        uint256 beforeOut = tokenA.balanceOf(recipient);

        executor.buySolidlyETH{value: amountIn}(
            address(solidlyRouter),
            address(wS),
            address(tokenA),
            false,
            amountIn,
            0,
            recipient,
            block.timestamp + 1,
            address(pair),
            1,
            1,
            0
        );

        uint256 afterOut = tokenA.balanceOf(recipient);
        assertEq(afterOut - beforeOut, 4e18, "buySolidlyETH output");
    }

    function test_sell_solidly_eth() public {
        uint256 amountIn = 1e18;
        uint256 beforeOut = recipient.balance;
        vm.deal(address(wS), 10 ether);

        executor.sellSolidlyETH(
            address(solidlyRouter),
            address(tokenA),
            address(wS),
            false,
            amountIn,
            0,
            recipient,
            block.timestamp + 1,
            address(pair),
            1,
            1,
            0
        );

        uint256 afterOut = recipient.balance;
        assertEq(afterOut - beforeOut, 1e18, "sellSolidlyETH output");
    }

    function test_sell_solidly_eth_clears_weth_balance() public {
        uint256 amountIn = 1e18;
        vm.deal(address(wS), 10 ether);

        executor.sellSolidlyETH(
            address(solidlyRouter),
            address(tokenA),
            address(wS),
            false,
            amountIn,
            0,
            recipient,
            block.timestamp + 1,
            address(pair),
            1,
            1,
            0
        );

        uint256 remaining = wS.balanceOf(address(executor));
        assertEq(remaining, 0, "wS balance cleared");
    }

    function test_default_recipient_is_owner() public {
        address[] memory path = new address[](2);
        path[0] = address(tokenA);
        path[1] = address(tokenB);
        uint256 beforeOut = tokenB.balanceOf(address(this));

        executor.buyV2(
            address(v2Router),
            path,
            1e18,
            0,
            address(0),
            block.timestamp + 1,
            address(pair),
            1,
            1,
            0
        );

        uint256 afterOut = tokenB.balanceOf(address(this));
        assertEq(afterOut - beforeOut, 2e18, "default recipient owner");
    }

    function test_only_owner() public {
        vm.prank(address(0xCAFE));
        vm.expectRevert(bytes("NOT_OWNER"));
        executor.setUseFeeOnTransfer(true);
    }

    function test_rescue_only_owner_reverts() public {
        vm.prank(address(0xCAFE));
        vm.expectRevert(bytes("NOT_OWNER"));
        executor.rescueToken(address(tokenA), recipient, 1e18);
    }

    function test_rescue_token_and_eth() public {
        tokenA.mint(address(executor), 100e18);
        vm.deal(address(executor), 2 ether);

        uint256 beforeToken = tokenA.balanceOf(recipient);
        executor.rescueToken(address(tokenA), recipient, 50e18);
        uint256 afterToken = tokenA.balanceOf(recipient);
        assertEq(afterToken - beforeToken, 50e18, "rescueToken amount");

        uint256 beforeEth = recipient.balance;
        executor.rescueETH(recipient, 1 ether);
        uint256 afterEth = recipient.balance;
        assertEq(afterEth - beforeEth, 1 ether, "rescueETH amount");
    }

    receive() external payable {}
}
