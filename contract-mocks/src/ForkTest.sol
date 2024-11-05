// SPDX-License-Identifier: UNLICENSED
pragma solidity ^0.8.13;

import {Credible, PhEvm} from "credible-std/Credible.sol";
import {Test} from "forge-std/Test.sol";

event Log(uint256 value);

Target constant target = Target(address(0x118DD24a3b0D02F90D8896E242D3838B4D37c181));

contract ForkTest is Credible, Test {
    uint256 expectedSum = 0;
    uint256 someInitValue = 1;

    constructor() payable {}

    function testForkSwitch() external {
        //Test fork switching reads from underlying state
        require(target.readStorage() == 2, "readStorage() != 2");

        ph.forkPreState();
        require(target.readStorage() == 1, "readStorage() != 1");

        ph.forkPostState();
        require(target.readStorage() == 2, "readStorage() != 2");

        ph.forkPreState();
        require(target.readStorage() == 1, "readStorage() != 1");
    }

    function testPersistTargetContracts() external {
        require(someInitValue == 1, "someInitValue != 1");
        uint256 sum = 0;

        //target.readStorage(); // Reading twice somehow breaks the test
        require(target.readStorage() == 2, "val != 2");
        expectedSum += target.readStorage();
        sum += target.readStorage();

        ph.forkPreState();
        require(target.readStorage() == 1, "readStorage != 1");
        expectedSum += target.readStorage();
        sum += target.readStorage();

        ph.forkPostState();
        require(target.readStorage() == 2, "val != 2");
        expectedSum += target.readStorage();
        sum += target.readStorage();

        ph.forkPreState();
        require(target.readStorage() == 1, "val != 1");
        expectedSum += target.readStorage();
        sum += target.readStorage();

        require(sum == expectedSum, "sum != expectedSum");
        require(sum == 6, "sum != 6");
    }
}

contract Target {
    uint256 value;

    constructor() payable {
        value = 1;
    }

    function readStorage() external view returns (uint256) {
        return value;
    }

    function writeStorage(uint256 value_) external {
        value = value_;
    }
}

contract TriggeringTx {
    constructor() payable {
        target.writeStorage(2);
    }
}
