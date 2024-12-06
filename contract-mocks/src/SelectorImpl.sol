// SPDX-License-Identifier: UNLICENSED
pragma solidity ^0.8.13;

interface Assertion {
    function fnSelectors() external returns (bytes4[] memory);
}

contract SelectorImpl is Assertion {
    event RunningAssertion(uint256 count);

    function assertCount() public {
        uint256 count = 0;

        if (count > 1) {
            revert("Counter cannot be greater than 1");
        }
    }

    function fnSelectors() external pure returns (bytes4[] memory selectors) {
        selectors = new bytes4[](3);
        selectors[0] = this.assertionStorage.selector;
        selectors[1] = this.assertionEther.selector;
        selectors[2] = this.assertionBoth.selector;
    }

    function assertionStorage() external returns (bool) {
        return true;
    }

    function assertionEther() external returns (bool) {
        return true;
    }

    function assertionBoth() external returns (bool) {
        return true;
    }
}

contract BadSelectorImpl {

    function fnSelectors() external pure returns (uint256 bad, bytes4[] memory selectors) { 
        selectors = new bytes4[](3);
        selectors[0] = this.assertionStorage.selector;
        selectors[1] = this.assertionEther.selector;
        selectors[2] = this.assertionBoth.selector;

    }

    function assertionStorage() external returns (bool) {
        return true;
    }

    function assertionEther() external returns (bool) {
        return true;
    }

    function assertionBoth() external returns (bool) {
        return true;
    }
}
