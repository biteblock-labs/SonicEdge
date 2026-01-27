// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

contract ZooLimitToken {
    string public name;
    string public symbol;
    uint8 public immutable decimals;

    uint256 public totalSupply;
    uint256 public maxTxAmount;
    uint256 public maxWalletAmount;
    uint256 public cooldownSeconds;

    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    event Transfer(address indexed from, address indexed to, uint256 value);
    event Approval(address indexed owner, address indexed spender, uint256 value);

    constructor(
        string memory name_,
        string memory symbol_,
        uint8 decimals_,
        uint256 totalSupply_,
        uint256 maxTxAmount_,
        uint256 maxWalletAmount_,
        uint256 cooldownSeconds_
    ) {
        name = name_;
        symbol = symbol_;
        decimals = decimals_;
        maxTxAmount = maxTxAmount_;
        maxWalletAmount = maxWalletAmount_;
        cooldownSeconds = cooldownSeconds_;
        _mint(msg.sender, totalSupply_);
    }

    function transfer(address to, uint256 value) external returns (bool) {
        return _transfer(msg.sender, to, value);
    }

    function approve(address spender, uint256 value) external returns (bool) {
        allowance[msg.sender][spender] = value;
        emit Approval(msg.sender, spender, value);
        return true;
    }

    function transferFrom(address from, address to, uint256 value) external returns (bool) {
        uint256 allowed = allowance[from][msg.sender];
        require(allowed >= value, "ALLOWANCE");
        if (allowed != type(uint256).max) {
            allowance[from][msg.sender] = allowed - value;
        }
        return _transfer(from, to, value);
    }

    function _transfer(address from, address to, uint256 value) internal returns (bool) {
        require(to != address(0), "ZERO");
        uint256 bal = balanceOf[from];
        require(bal >= value, "BAL");
        unchecked {
            balanceOf[from] = bal - value;
            balanceOf[to] += value;
        }
        emit Transfer(from, to, value);
        return true;
    }

    function _mint(address to, uint256 value) internal {
        totalSupply += value;
        balanceOf[to] += value;
        emit Transfer(address(0), to, value);
    }
}
