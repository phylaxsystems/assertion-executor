// SPDX-License-Identifier: UNLICENSED
pragma solidity ^0.8.28;

import {Assertion} from "credible-std/Assertion.sol";
import {Test} from "forge-std/Test.sol";

import {Target, TARGET_ADDRESS} from "./Target.sol";

contract TestFork is Assertion, Test {
    uint256 expectedSum = 0;
    uint256 someInitValue = 1;

    constructor() payable {}

    function testForkSwitch() external {
        //Test fork switching reads from underlying state
        require(TARGET_ADDRESS.readStorage() == 2, "readStorage() != 2");

        ph.forkPreState();
        require(TARGET_ADDRESS.readStorage() == 1, "readStorage() != 1");

        ph.forkPostState();
        require(TARGET_ADDRESS.readStorage() == 2, "readStorage() != 2");

        ph.forkPreState();
        require(TARGET_ADDRESS.readStorage() == 1, "readStorage() != 1");
    }

    function testPersistTargetContracts() external {
        require(someInitValue == 1, "someInitValue != 1");
        uint256 sum = 0;

        require(TARGET_ADDRESS.readStorage() == 2, "val != 2");
        expectedSum += TARGET_ADDRESS.readStorage();
        sum += TARGET_ADDRESS.readStorage();

        ph.forkPreState();
        require(TARGET_ADDRESS.readStorage() == 1, "readStorage != 1");
        expectedSum += TARGET_ADDRESS.readStorage();
        sum += TARGET_ADDRESS.readStorage();

        ph.forkPostState();
        require(TARGET_ADDRESS.readStorage() == 2, "val != 2");
        expectedSum += TARGET_ADDRESS.readStorage();
        sum += TARGET_ADDRESS.readStorage();

        ph.forkPreState();
        require(TARGET_ADDRESS.readStorage() == 1, "val != 1");
        expectedSum += TARGET_ADDRESS.readStorage();
        sum += TARGET_ADDRESS.readStorage();

        require(sum == expectedSum, "sum != expectedSum");
        require(sum == 6, "sum != 6");
    }

    function triggers() external view override {
        registerCallTrigger(this.testForkSwitch.selector);
        registerCallTrigger(this.testPersistTargetContracts.selector);
    }
}

contract TriggeringTx {
    constructor() payable {
        TARGET_ADDRESS.writeStorage(2);
    }
}
