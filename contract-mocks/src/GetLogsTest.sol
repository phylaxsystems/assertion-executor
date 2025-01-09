// SPDX-License-Identifier: UNLICENSED
pragma solidity ^0.8.13;

import {Credible, PhEvm} from "credible-std/Credible.sol";
import {Test} from "forge-std/Test.sol";
import {console} from "forge-std/console.sol";

Target constant target = Target(address(0x118DD24a3b0D02F90D8896E242D3838B4D37c181));

contract GetLogsTest is Credible, Test {
    constructor() payable {}

    function testGetLogs() external {
        require(target.readStorage() == 1, "val != 1");
        PhEvm.Log[] memory logs = ph.getLogs();
        require(logs.length == 1, "logs.length != 1");
        PhEvm.Log memory log = logs[0];
        require(log.emitter == address(target), "log.address != target");
        require(log.topics.length == 1, "log.topics.length != 1");
        require(log.topics[0] == Target.Log.selector, "log.topics[0] != Target.Log.selector");
        require(log.data.length == 32, "log.data.length != 32");
        require(bytes32(log.data) == bytes32(uint256(1)), "log.data != 1");
    }

    function fnSelectors() external pure returns (bytes4[] memory selectors) {
        selectors = new bytes4[](1);
        selectors[0] = this.testGetLogs.selector;
    }
}

contract Target {
    event Log(uint256 value);

    uint256 value;

    constructor() payable {}

    function readStorage() external view returns (uint256) {
        return value;
    }

    function writeStorage(uint256 value_) external {
        value = value_;
        emit Log(value);
    }
}

contract TriggeringTx {
    constructor() payable {
        target.writeStorage(1);
    }
}
