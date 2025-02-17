// SPDX-License-Identifier: UNLICENSED
pragma solidity ^0.8.13;

import {Assertion} from "credible-std/Assertion.sol";
import {PhEvm} from "credible-std/PhEvm.sol";
import {Test} from "forge-std/Test.sol";
import {console} from "forge-std/console.sol";

import {Target, TARGET_ADDRESS} from "./Target.sol";

contract TestGetCallInputs is Assertion, Test {
    constructor() payable {}

    function testGetCallInputs() external view {
        PhEvm.CallInputs[] memory callInputs = ph.getCallInputs(address(TARGET_ADDRESS), Target.readStorage.selector);
        require(callInputs.length == 1, "callInputs.length != 1");
        PhEvm.CallInputs memory callInput = callInputs[0];

        require(callInput.target_address == address(TARGET_ADDRESS), "callInput.target_address != target");
        require(callInput.input.length == 4, "callInput.input.length != 4");
        require(bytes4(callInput.input) == Target.readStorage.selector, "callInput.input != readStorage()");
        require(callInput.value == 0, "callInput.value != 0");

        callInputs = ph.getCallInputs(address(TARGET_ADDRESS), Target.writeStorage.selector);
        require(callInputs.length == 2, "callInputs.length != 2");

        callInput = callInputs[0];
        require(callInput.target_address == address(TARGET_ADDRESS), "callInput.target_address != target");
        require(callInput.input.length == 36, "callInput.input.length != 36");
        require(bytes4(callInput.input) == Target.writeStorage.selector, "callInput.input != writeStorage(uint256)");
        uint256 param = abi.decode(stripSelector(callInput.input), (uint256));
        require(param == 1, "First writeStorage param should be 1");
        require(callInput.value == 0, "callInput.value != 0");

        callInput = callInputs[1];
        require(callInput.target_address == address(TARGET_ADDRESS), "callInput.target_address != target");
        require(callInput.input.length == 36, "callInput.input.length != 36");
        require(bytes4(callInput.input) == Target.writeStorage.selector, "callInput.input != writeStorage(uint256)");
        param = abi.decode(stripSelector(callInput.input), (uint256));
        require(param == 2, "Second writeStorage param should be 2");
        require(callInput.value == 0, "callInput.value != 0");
    }

    function stripSelector(bytes memory input) internal pure returns (bytes memory) {
        // Create a new bytes memory and copy everything after the selector
        bytes memory paramData = new bytes(32);
        for (uint256 i = 4; i < input.length; i++) {
            paramData[i - 4] = input[i];
        }
        return paramData;
    }

    function triggers() external view override {
        registerCallTrigger(this.testGetCallInputs.selector);
    }
}

contract TriggeringTx {
    constructor() payable {
        TARGET_ADDRESS.writeStorage(1);
        TARGET_ADDRESS.writeStorage(2);
        TARGET_ADDRESS.readStorage();
    }
}
