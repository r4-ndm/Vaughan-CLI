// SPDX-License-Identifier: MIT
pragma solidity 0.8.23;

/// Minimal fixed-supply ERC-20 for Vaughan testnet meme-coin launches.
/// Full supply minted once in the constructor to `recipient` — no owner mint.
contract FixedSupplyToken {
    string public name;
    string public symbol;
    uint8 public immutable decimals;
    uint256 public totalSupply;

    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    event Transfer(address indexed from, address indexed to, uint256 value);
    event Approval(address indexed owner, address indexed spender, uint256 value);

    constructor(
        string memory name_,
        string memory symbol_,
        uint8 decimals_,
        uint256 initialSupply_,
        address recipient_
    ) {
        require(recipient_ != address(0), "recipient=0");
        require(initialSupply_ > 0, "supply=0");
        name = name_;
        symbol = symbol_;
        decimals = decimals_;
        totalSupply = initialSupply_;
        balanceOf[recipient_] = initialSupply_;
        emit Transfer(address(0), recipient_, initialSupply_);
    }

    function transfer(address to, uint256 amount) external returns (bool) {
        _transfer(msg.sender, to, amount);
        return true;
    }

    function approve(address spender, uint256 amount) external returns (bool) {
        allowance[msg.sender][spender] = amount;
        emit Approval(msg.sender, spender, amount);
        return true;
    }

    function transferFrom(address from, address to, uint256 amount) external returns (bool) {
        uint256 allowed = allowance[from][msg.sender];
        if (allowed != type(uint256).max) {
            allowance[from][msg.sender] = allowed - amount;
        }
        _transfer(from, to, amount);
        return true;
    }

    function _transfer(address from, address to, uint256 amount) internal {
        require(to != address(0), "to=0");
        balanceOf[from] -= amount;
        balanceOf[to] += amount;
        emit Transfer(from, to, amount);
    }
}
