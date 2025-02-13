contract SSTOREGas {
    fallback() external payable {
        assembly {
            sstore(0x0, 0x1)
        }
    }

    function get() external view returns (uint256) {
        uint256 val;
        assembly {
            val := sload(0x0)
        }
        return val;
    }
}

contract SLOADGas {
    fallback() external payable {
        assembly {
            let val := sload(0x0)
            log1(val, 0, 0)
        }
    }
    function set(uint256 val) external {
        assembly {
            sstore(0x0, val)
        }
    }
}
