// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

import {Credible, PhEvm} from "credible-std/Credible.sol";
import {Test} from "forge-std/Test.sol";

DemoLending constant target = DemoLending(address(0x118DD24a3b0D02F90D8896E242D3838B4D37c181));

contract DemoLending {
    mapping(address => uint256) public balances;
    mapping(address => uint256) public borrowed;

    event Deposited(address indexed user, uint256 amount);
    event Withdrawn(address indexed user, uint256 amount);
    event Borrowed(address indexed user, uint256 amount);
    event Repaid(address indexed user, uint256 amount);

    function deposit() public payable {
        require(msg.value > 0, "Must deposit some ETH");
        balances[msg.sender] = balances[msg.sender] + msg.value;
        emit Deposited(msg.sender, msg.value);
    }

    function withdraw(uint256 _amount) public {
        // Vulnerability: No check for outstanding borrowed amounts and balances

        balances[msg.sender] = balances[msg.sender] - _amount;
        (bool sent,) = msg.sender.call{value: _amount}("");
        require(sent, "Failed to send ETH");

        emit Withdrawn(msg.sender, _amount);
    }

    function borrow(uint256 _amount) public {
        uint256 maxBorrow = balances[msg.sender] * 9 / 10;
        // Vulnerability: Check borrowed amount AFTER transfer
        (bool sent,) = msg.sender.call{value: _amount}("");
        require(sent, "Failed to send ETH");

        borrowed[msg.sender] = borrowed[msg.sender] + _amount;

        emit Borrowed(msg.sender, _amount);
    }

    function repay() public payable {
        require(msg.value > 0, "Must repay some amount");
        require(borrowed[msg.sender] >= msg.value, "Repaying too much");

        borrowed[msg.sender] = borrowed[msg.sender] - msg.value;
        emit Repaid(msg.sender, msg.value);
    }

    function getDeposit() public view returns (uint256) {
        return balances[msg.sender];
    }

    function getDebt() public view returns (uint256) {
        return borrowed[msg.sender];
    }
}

contract NormalTx {
    constructor() payable {
        target.deposit{value: 10 ether}();
    }
}

contract TriggeringTx {
    constructor() payable {
        target.deposit{value: 10 ether}();
        target.borrow(5 ether);
        target.withdraw(10 ether);
    }
}

contract DemoLendingAssertion is Credible, Test {

    function testWithdraw() public {
        uint256 balance_now = address(msg.sender).balance;

        uint256 deposit_before;
        uint256 borrow_after;

        ph.forkPreState();
        deposit_before = target.getDeposit();
        borrow_after = target.getDebt();

        require(balance_now <= (deposit_before + borrow_after), "Insufficient balance");
    }
}
