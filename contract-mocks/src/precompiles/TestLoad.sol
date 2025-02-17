// SPDX-License-Identifier: UNLICENSED
pragma solidity ^0.8.13;

import {Assertion} from "credible-std/Assertion.sol";
import {PhEvm} from "credible-std/PhEvm.sol";
import {Test} from "forge-std/Test.sol";
import {console} from "forge-std/console.sol";

import {Target, TARGET_ADDRESS} from "./Target.sol";

contract TestLoad is Assertion, Test {
    constructor() payable {}

    function testLoad() external view {
        // Test reading existing slot
        bytes32 loaded = ph.load(address(TARGET_ADDRESS), 0);
        require(uint256(loaded) == 1);

        // Test reading non-existing slot, as bytes32
        loaded = ph.load(address(TARGET_ADDRESS), bytes32(uint256(1)));
        require(uint256(loaded) == 0);
    }

    function triggers() external view override {
      registerCallTrigger(this.testLoad.selector);
    }
}

contract TriggeringTx {
    constructor() payable {
        TARGET_ADDRESS.writeStorage(1);
    }
}
