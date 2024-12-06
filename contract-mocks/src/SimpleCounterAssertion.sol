// SPDX-License-Identifier: UNLICENSED
pragma solidity ^0.8.13;

contract Counter {
    uint256 public number;

    function increment() public {
        number++;
    }
}

contract SimpleCounterAssertion {
    event RunningAssertion(uint256 count);

    function assertCount() public {
        uint256 count = Counter(0x0101010101010101010101010101010101010101).number();
        emit RunningAssertion(count);
        if (count > 1) {
            revert("Counter cannot be greater than 1");
        }
    }

    function fnSelectors() external pure returns (bytes4[] memory selectors) {
        selectors = new bytes4[](1);
        selectors[0] = this.assertCount.selector;
    }
}
