// SPDX-License-Identifier: CC0-1.0
pragma solidity 0.8.23;

/// Canonical ERC-5564 announcer (ScopeLift / EIP-5564).
/// CREATE2: factory 0x4e59b44847b379578588920cA78FbF26c0B4956C
/// salt    0xd0103a290d760f027c9ca72675f5121d725397fb2f618f05b6c44958b25b4447
/// address 0x55649E01B5Df198D18D95b5cc5051630cfD45564
contract ERC5564Announcer {
    event Announcement(
        uint256 indexed schemeId,
        address indexed stealthAddress,
        address indexed caller,
        bytes ephemeralPubKey,
        bytes metadata
    );

    function announce(
        uint256 schemeId,
        address stealthAddress,
        bytes memory ephemeralPubKey,
        bytes memory metadata
    ) external {
        emit Announcement(schemeId, stealthAddress, msg.sender, ephemeralPubKey, metadata);
    }
}
