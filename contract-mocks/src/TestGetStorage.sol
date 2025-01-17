// SPDX-License-Identifier: UNLICENSED
pragma solidity ^0.8.13;

import {Credible, PhEvm} from "credible-std/Credible.sol";
import {Test} from "forge-std/Test.sol";
import {console} from "forge-std/console.sol";

Target constant target = Target(address(0x118DD24a3b0D02F90D8896E242D3838B4D37c181));

contract GetStorageTest is Credible, Test {
    constructor() payable {}

    function testLoad() external {
        // Test reading existing slot
        bytes32 loaded = ph.load(address(target), 0);
        require(uint256(loaded) == 1);

        // Test reading non-existing slot, as bytes32
        loaded = ph.load(address(target), bytes32(uint256(1)));
        require(uint256(loaded) == 0);
    }

    function fnSelectors() external pure returns (bytes4[] memory selectors) {
        selectors = new bytes4[](1);
        selectors[0] = this.testLoad.selector;
    }
}

contract Target {
    event Log(uint256 value);

    uint256 value;

    constructor() payable {
        value = 1;
    }

    function readStorage() external returns (uint256) {
        return value;
    }
}

contract TriggeringTx {
    constructor() payable {
        target.readStorage();
    }
}
