# Changelog

## [0.2.0](https://github.com/Onely7/qdu/compare/v0.1.0...v0.2.0) (2026-11-02)


### ⚠ BREAKING CHANGES

* the unlock command is removed; locks need no manual cleanup.
* **output:** `diff` no longer lists unchanged paths in any format.
* **cli:** use `check --under` instead of `check --path`, drop `diff --previous`, and pass `--older-than 30d` to keep the old manual compaction cutoff.
* **retention:** new profiles keep 52 snapshots instead of 100, and snapshots older than seven days are compressed automatically; set `compact_after = off` to keep them uncompressed.
* **diff:** diff JSON entries use `previous`, `current`, `change`, and `change_percent` instead of `previous_bytes`, `current_bytes`, and `change_bytes`, and add a top-level `metric`; TSV columns change to rank, metric, change, change_percent, current, previous, and path.
* slash patterns are anchored to the scan root instead of matching path suffixes, and backslashes escape characters. Snapshots taken with different syntax report differing exclusion patterns in `qdu diff`.

### Features

* **browse:** list directories largest first with their size ([1aec3f8](https://github.com/Onely7/qdu/commit/1aec3f8f5c8dc7b0259adaaec24ae9cb0f2ecb35))
* **check:** add --snapshot-age-over ([b2471d0](https://github.com/Onely7/qdu/commit/b2471d045f3fdb612cd252c4e5bf2a2ef7165dab))
* **cli:** describe every option and align option names ([81e61b2](https://github.com/Onely7/qdu/commit/81e61b2c8a44b73b585a82e0399d50e5624f02e7))
* **compact:** report the result in a sentence ([71e7fd2](https://github.com/Onely7/qdu/commit/71e7fd2581312d507e6131dcb14c85b51ad5c4de))
* **diff:** compare apparent bytes or inodes with --metric ([5540c5f](https://github.com/Onely7/qdu/commit/5540c5f6fb199873089361f35b4710270d918618))
* **doctor:** report the newest snapshot, the root, and the cron entry ([0bcb0aa](https://github.com/Onely7/qdu/commit/0bcb0aa6356b9196faba332511c34141d1a676f7))
* **list:** show the disk space each snapshot uses ([8264f5e](https://github.com/Onely7/qdu/commit/8264f5e6c6f59722caa4516b5bfba3efbb7e2376))
* **output:** give files and inodes the same header as show ([c7944dd](https://github.com/Onely7/qdu/commit/c7944ddd2ffd0f06461298bec72c76d99540a992))
* **output:** line plain table headers up with their columns ([08f74e9](https://github.com/Onely7/qdu/commit/08f74e98099486ada1e45ce07e834417df5321f7))
* **output:** round ages to their two largest units ([9f7fb67](https://github.com/Onely7/qdu/commit/9f7fb673227ca244965a2d915b867f0afb966e81))
* **output:** shorten report headers and list only changes in diffs ([f116bd7](https://github.com/Onely7/qdu/commit/f116bd774f9c5152d88be4d8d472966050ba697d))
* **profile:** show settings as configuration text and list snapshot counts ([07d22ea](https://github.com/Onely7/qdu/commit/07d22ea82025e50f4980333b0fbfd305d3760eec))
* remove qdu unlock and describe lock holders in words ([b490215](https://github.com/Onely7/qdu/commit/b490215f22b05e1dccef7432d290f32cb528bf53))
* **retention:** compress older snapshots automatically ([86b8ab3](https://github.com/Onely7/qdu/commit/86b8ab3ba7555ae80f1c0888b410c9434b6cc24d))
* **schedule:** set up periodic snapshots with cron ([77cccb4](https://github.com/Onely7/qdu/commit/77cccb4fce3840b918843d81fd3064222bc15125))
* **snapshot:** show scan progress on terminals ([c313105](https://github.com/Onely7/qdu/commit/c313105aa9b564bbeff8aac07f9156789807dd67))
* **units:** accept du-style size suffixes such as 2T ([23a2068](https://github.com/Onely7/qdu/commit/23a206876b746684ce72f526f7020a900ed6103f))
* use gitignore syntax for exclusion patterns ([78fa4b7](https://github.com/Onely7/qdu/commit/78fa4b70852565b0baa545bd0d8ae2e61a0af9f2))


### Bug Fixes

* **browse:** pass paths to fzf as NUL-terminated records ([cbca349](https://github.com/Onely7/qdu/commit/cbca34964adadc0dc8d8f3969b2fabab030be082))
* **check:** reject growth paths absent from both snapshots ([51d0ac9](https://github.com/Onely7/qdu/commit/51d0ac9b0ada2c2bdcb374e50d81f002c69fcccf))
* **diff:** explain that a comparison needs two complete snapshots ([ee54d52](https://github.com/Onely7/qdu/commit/ee54d523a542a294c264775b608fef18746f0af2))
* **explain:** reject --growth-only together with --shrink-only ([a13a8a1](https://github.com/Onely7/qdu/commit/a13a8a121a40e365987134489c595cf8aef32430))
* **files:** count live allocations the way snapshots do ([5880cbd](https://github.com/Onely7/qdu/commit/5880cbdae21332281b12dcf94c91039c12d1d27a))
* **files:** let live scans cross filesystems when the profile does ([c3d8d0a](https://github.com/Onely7/qdu/commit/c3d8d0a7a209c2dfb72aaf9b3b3edfa86a54a13d))
* **output:** use the singular for a count of one ([2e1292b](https://github.com/Onely7/qdu/commit/2e1292b155b5df7989822670ecf4e1ecdf6500c6))
* **profile:** keep omitted settings when adding a profile ([9478dcc](https://github.com/Onely7/qdu/commit/9478dccb75b392fff01925aa82bab8445013b440))
* **profile:** list valid profiles when another one is invalid ([588d2f7](https://github.com/Onely7/qdu/commit/588d2f7576570aace55ea29d9730bd2231b6cd28))
* **query:** match --under scopes case-sensitively ([041aa38](https://github.com/Onely7/qdu/commit/041aa38c57064be6cb9be89be58fe2fed1d4b3db))
* **render:** report empty tables in English ([5f58be0](https://github.com/Onely7/qdu/commit/5f58be05dbe6cd4af6b1bda2f9e6c0e2114f0267))
* **scan:** bound open directory handles in deep trees ([c0588f6](https://github.com/Onely7/qdu/commit/c0588f626b2e7ff59b59c5a1c050c1caedafdce1))
* **scan:** record directory listing failures instead of aborting ([6183f89](https://github.com/Onely7/qdu/commit/6183f896b9471169a87cc8492038ad420c02477c))
* **scan:** store undecodable file names instead of crashing ([efedb16](https://github.com/Onely7/qdu/commit/efedb16333fe94d14469c84a60e3b692c39d28f4))
* **show:** honor --users in JSON output and require owner data ([6c93079](https://github.com/Onely7/qdu/commit/6c93079255842025ce3e19c0f585b0cc16ad67c4))
* **show:** warn when a capacity limit cannot be evaluated ([09cac58](https://github.com/Onely7/qdu/commit/09cac58da6b58a3a6c2602b99cb2dc93e6d5926f))
* **snapshot:** say how to resolve a root mismatch ([09d3dcf](https://github.com/Onely7/qdu/commit/09d3dcfe81b3714f7f76ac2f7d45b482980bae97))
* **storage:** expand archives in the system temporary directory ([ee4a9c9](https://github.com/Onely7/qdu/commit/ee4a9c95e1e8f6cb6086c104a93f15d4c2ecb1ee))
* **storage:** save the index before deleting snapshot files ([32db3e1](https://github.com/Onely7/qdu/commit/32db3e1916fb891372846497be6a5d90382ca725))
* **units:** reject unknown size suffixes instead of crashing ([a3df066](https://github.com/Onely7/qdu/commit/a3df06698add2038df413eef2d8878e4ca5d6df6))


### Performance Improvements

* **build:** ship bytecode in the zipapp ([437af6a](https://github.com/Onely7/qdu/commit/437af6a9c16470439881cdfcf20cce3ce66a5699))
* **scan:** aggregate owner statistics in memory ([8802ec1](https://github.com/Onely7/qdu/commit/8802ec12ecc4f0900b34aff0bd1fdadd5a5dda49))
* **scan:** count each directory's files in one tight loop ([075ccc6](https://github.com/Onely7/qdu/commit/075ccc664050e72a353304fca2864259c1f63136))
* **scan:** precompile exclusion patterns ([575699a](https://github.com/Onely7/qdu/commit/575699af80b29a4e1e3da5466dc702253966ec92))
* **schedule:** recognize the zipapp without importing zipfile ([8a43b76](https://github.com/Onely7/qdu/commit/8a43b767352e6f10a0539f922ff6ccd06f1e5320))
* **storage:** write compact schema-3 snapshots ([28f5224](https://github.com/Onely7/qdu/commit/28f5224f6856f71c8c0567a8f4db4b6aeaa6f2f3))


### Documentation

* build the zipapp before installing in the Japanese guides ([7acabfb](https://github.com/Onely7/qdu/commit/7acabfb303833e04e925b1167fe72203a4b6f149))
* **cli:** show the default of --compact-after in its help ([b9cf024](https://github.com/Onely7/qdu/commit/b9cf024e44f9764f5a32755e64835cd95c8483f2))
* describe the benchmark machine without server details ([084bd40](https://github.com/Onely7/qdu/commit/084bd404f31a3e13b39606c096dfd475c31a1c1f))
* explain how file names that are not UTF-8 are stored ([6d7a067](https://github.com/Onely7/qdu/commit/6d7a067c557153160d78ff8364e0ee87b931e986))
* **ja:** explain the busy-profile error in troubleshooting ([4c6dfc9](https://github.com/Onely7/qdu/commit/4c6dfc982026fda92e300c892838602363f8d1ef))
* **ja:** fix the reference wording and the skipped-mount output ([6686c4a](https://github.com/Onely7/qdu/commit/6686c4a95a49bc0b4cfb3ec9933f2e76327f794a))
* **ja:** polish the analysis guide ([23d0311](https://github.com/Onely7/qdu/commit/23d031137ba1995f7f60489c0560e186abf67eb6))
* **ja:** polish the command reference introduction ([a5da90a](https://github.com/Onely7/qdu/commit/a5da90a472af874bad3009b8b21d09b1d95f2965))
* **ja:** polish the configuration and storage guide ([3ebb693](https://github.com/Onely7/qdu/commit/3ebb693e5801a7eb9703933c11cb142365da4ad9))
* **ja:** polish the development guide ([8bd4240](https://github.com/Onely7/qdu/commit/8bd424032bd013ea93131104c7541077c330915a))
* **ja:** polish the getting started and snapshot guides ([5191b3c](https://github.com/Onely7/qdu/commit/5191b3cdabc70a830fa5e8476536d8bf691de346))
* **ja:** polish the operations and troubleshooting guides ([ec74ff0](https://github.com/Onely7/qdu/commit/ec74ff08b7bb1320e6996604830ea1a8c204eb37))
* **ja:** polish the README and the documentation index ([9005161](https://github.com/Onely7/qdu/commit/900516170e283a9b5e7a35295520cb11ccae95b6))
* **ja:** polish the tmux scheduling guide ([cd5831f](https://github.com/Onely7/qdu/commit/cd5831f5068ddffc7b29c8a3193669933fd34f2a))
* **ja:** show capacity_limit as qdu now stores it ([21ba8b8](https://github.com/Onely7/qdu/commit/21ba8b8a2e1e6f56fe5f35aa4d98300d8c67fd50))
* **ja:** use a neutral filesystem size in the capacity example ([2d9dc59](https://github.com/Onely7/qdu/commit/2d9dc59270bcd2306b2ea12227040af2d3d9a53a))
* point to release downloads and to qdu schedule ([7fb4e65](https://github.com/Onely7/qdu/commit/7fb4e65cc93e86d35f10e9830279d110fab3e51b))
* say how a diff shows paths that did not exist before ([8615a6c](https://github.com/Onely7/qdu/commit/8615a6cab3c356047fb396f57477f7c248423a37))
* use generic user names in examples ([2b703d3](https://github.com/Onely7/qdu/commit/2b703d37ca6b7096edeebdc2f8167deeb5dc82c5))

## [0.1.0](https://github.com/Onely7/qdu/compare/v0.0.1...v0.1.0) (2026-07-19)


### Features

* add Release Please automation ([db49045](https://github.com/Onely7/qdu/commit/db490456b3425af9305682016bf349a025217d98))


### Documentation

* add README preview image ([bf174d6](https://github.com/Onely7/qdu/commit/bf174d675a833425a9651e72164a05f538a5e1b8))
* add README preview image ([89db5bf](https://github.com/Onely7/qdu/commit/89db5bfa592bd84c767066dbd539b742e5b8c4d3))
