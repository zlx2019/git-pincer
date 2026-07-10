# Changelog

All notable changes to this project will be documented in this file. See [conventional commits](https://www.conventionalcommits.org/) for commit guidelines.

---
## [0.1.2](https://github.com/zlx2019/git-pincer/compare/v0.1.1..v0.1.2) - 2026-07-10

### Bug Fixes

- **(ui)** keep alternate screen alive from menu into conflict UI - ([3dd001f](https://github.com/zlx2019/git-pincer/commit/3dd001fee81e3f0be55008c6a49d62f54caf3dbe)) - Zero
- **(ui)** defer the running page and drop the pre-clear blank frame - ([8f2baee](https://github.com/zlx2019/git-pincer/commit/8f2baeec4fa43d8e80ae9ed6051d14d3da4ae518)) - Zero
- **(ui)** cross-platform clipboard, editor fallback and Ctrl+C - ([2075136](https://github.com/zlx2019/git-pincer/commit/20751361f8ef0ecacb535b4fcb9daaf6b857b07e)) - Zero
- **(ui)** respect $VISUAL before $EDITOR when picking the editor - ([c8e8627](https://github.com/zlx2019/git-pincer/commit/c8e862721b9ad0067a3491ce1c16f7bb7a2c1aaf)) - Zero

### Documentation

- note the one-time shell hook for tab completion - ([a47a7c9](https://github.com/zlx2019/git-pincer/commit/a47a7c93dcde82eb3df99df8c735369a9dfa5b12)) - Zero
- split completion setup into per-shell commands - ([572873e](https://github.com/zlx2019/git-pincer/commit/572873e3027fa1236d27f03d9ad09bc94a007c9c)) - Zero

### Features

- **(cli)** completions subcommand for shell completion scripts - ([bb133a3](https://github.com/zlx2019/git-pincer/commit/bb133a316cf8d18d3ed4f3f2d91e575f5a05610c)) - Zero
- **(config)** user config file for theme, key bindings and option defaults - ([ea30a5e](https://github.com/zlx2019/git-pincer/commit/ea30a5efd15d94902c2bb99a1d7af466e4ff510f)) - Zero
- **(config)** configurable editor with full fallback chain - ([7d5e8d6](https://github.com/zlx2019/git-pincer/commit/7d5e8d65294071725592780c54f98cb8a5208e47)) - Zero
- **(ui)** free viewport scrolling with Ctrl+d / Ctrl+u - ([da45bdd](https://github.com/zlx2019/git-pincer/commit/da45bdd630b7e6b609e73346d0cf056329c85ef5)) - Zero

### Miscellaneous Chores

- **(meta)** crates.io categories, version badge, changelog and API note - ([bcc4ad5](https://github.com/zlx2019/git-pincer/commit/bcc4ad58dd6d2397a254b3766d91590d42fd74b8)) - Zero

### Other

- Merge pull request #3 from zlx2019/perf/hot-path

perf: hot-path optimizations and seamless menu-to-resolve transition - ([59aeba3](https://github.com/zlx2019/git-pincer/commit/59aeba37a2cc0c8739e8f059908fc5a48b75849e)) - Zero
- Merge pull request #4 from zlx2019/refactor/keymap

refactor: single source of truth for key bindings - ([ee8ecd5](https://github.com/zlx2019/git-pincer/commit/ee8ecd5e8e2b61dbc021c9e0fcbdea5bed0c28e4)) - Zero
- Merge pull request #5 from zlx2019/feat/config

feat: user config file for theme, key bindings and option defaults - ([15499a2](https://github.com/zlx2019/git-pincer/commit/15499a2c4e038da691725a3e578397878d87d53b)) - Zero
- run tests on ubuntu, macos and windows - ([59c2307](https://github.com/zlx2019/git-pincer/commit/59c2307d3c84b1d2ae00e5ddedfe8b39642cd869)) - Zero
- Merge pull request #6 from zlx2019/fix/compat

fix: cross-platform compatibility (clipboard, editor, Ctrl+C) + CI matrix - ([de5bfb4](https://github.com/zlx2019/git-pincer/commit/de5bfb440902dcc1ae5d5f1152f90e678ab571b4)) - Zero
- Merge pull request #7 from zlx2019/fix/compat

feat: configurable editor with full fallback chain - ([5db7b18](https://github.com/zlx2019/git-pincer/commit/5db7b1866431b089ba46b919d047821842ed87e0)) - Zero
- Merge pull request #8 from zlx2019/feat/scroll

feat: free viewport scrolling with Ctrl+d / Ctrl+u - ([bf909e5](https://github.com/zlx2019/git-pincer/commit/bf909e50bb583a639103c2e42e8080f4902e6f31)) - Zero
- Merge pull request #9 from zlx2019/chore/polish

chore: release polish — shell completions, categories, badge, changelog - ([5954946](https://github.com/zlx2019/git-pincer/commit/5954946ce6f1dbf7c77cd0212bd6dbc0c7b6e68f)) - Zero
- Merge pull request #10 from zlx2019/chore/polish

docs: split completion setup into per-shell commands - ([9b4c67f](https://github.com/zlx2019/git-pincer/commit/9b4c67f4a11e5d0cb5b75119b862a7c425b1be28)) - Zero

### Performance

- **(git)** batch-read conflict blobs with a single cat-file process - ([0ed6fdc](https://github.com/zlx2019/git-pincer/commit/0ed6fdcb8165597c2d2d5a44256781c4fe0c5fe1)) - Zero
- **(git)** probe repo vitals with parallel queries - ([091ee10](https://github.com/zlx2019/git-pincer/commit/091ee1057cab85278fa55cb74dcce6fc7b90a633)) - Zero
- **(merge)** stop triple-storing stable chunk content - ([bc37c33](https://github.com/zlx2019/git-pincer/commit/bc37c33377ae371f331413d5d9fd2fa3ba029fe7)) - Zero
- **(ui)** rehighlight result pane incrementally by chunk - ([ba96e27](https://github.com/zlx2019/git-pincer/commit/ba96e27c31cb9b6cd79cf8d6b7fdd995d3a0d6d3)) - Zero
- **(ui)** cache render rows keyed by (file, revision, folded) - ([bc5d15a](https://github.com/zlx2019/git-pincer/commit/bc5d15ae0f17c5d8e6bf50e188b52ded57036fca)) - Zero
- **(ui)** build the three initial highlight panes in parallel - ([3be1825](https://github.com/zlx2019/git-pincer/commit/3be1825d7302ccfaa06f3f6b810a69f8c1cb6349)) - Zero

### Refactoring

- **(ui)** single source of truth for key bindings - ([160acde](https://github.com/zlx2019/git-pincer/commit/160acdecd9b5be7fe72ffd6ae456e5f050978017)) - Zero

---
## [0.1.1](https://github.com/zlx2019/git-pincer/compare/v0.1.0..v0.1.1) - 2026-07-09

### Bug Fixes

- print clean one-line errors without backtrace - ([d59114a](https://github.com/zlx2019/git-pincer/commit/d59114acc5f60be21041457b8e9f31d20ec54f40)) - Zero

---
## [0.1.0] - 2026-07-09

### Bug Fixes

- **(ui)** run menus in one TUI session to stop flicker and keep cursor - ([a2109bd](https://github.com/zlx2019/git-pincer/commit/a2109bd47d5b36a888a7a45c78c7dadd95c8e2f9)) - Zero
- restore y/N default convention in abort confirmation prompt - ([6b605d0](https://github.com/zlx2019/git-pincer/commit/6b605d0e6083638120ceae6dfaab7237e2f7e76e)) - Zero
- report conflict count correctly in file mode message - ([735ca20](https://github.com/zlx2019/git-pincer/commit/735ca2089c106d4d161d9d271626966e00fcdb9e)) - Zero
- correct pass-through claims and wording in CLI help - ([7c93759](https://github.com/zlx2019/git-pincer/commit/7c937592c72b860b9d81287716f016fb20944ddd)) - Zero

### Documentation

- rewrite bilingual README and tidy package metadata - ([3f2e06e](https://github.com/zlx2019/git-pincer/commit/3f2e06e2d9838d3b2ccb4425f6d566f3154b27d1)) - Zero
- add logo, demo screenshot and naming story to README - ([93d3d51](https://github.com/zlx2019/git-pincer/commit/93d3d5133507f870b7531fb57388f4f5616a97fa)) - Zero
- refresh README for RPG menu, i18n and new showcase image - ([9a7fd34](https://github.com/zlx2019/git-pincer/commit/9a7fd34f3c2d098e3c3511a9e43ae486ae59f157)) - Zero

### Features

- **(i18n)** bilingual UI (zh/en) with system locale detection - ([7e7b1d0](https://github.com/zlx2019/git-pincer/commit/7e7b1d0cedb1649c06c4c2e355b77b32d744e12f)) - Zero
- **(ui)** hybrid-style visual overhaul for the three-pane TUI - ([a8ab7a2](https://github.com/zlx2019/git-pincer/commit/a8ab7a25d968668333ea9abe62395c65438c1049)) - Zero
- **(ui)** light theme with terminal-aware color handling - ([bddea61](https://github.com/zlx2019/git-pincer/commit/bddea6130a62c286cd5400752b944d8176a720e2)) - Zero
- **(ui)** undo-all key, deeper dark bands and continuous word emphasis - ([1914ea0](https://github.com/zlx2019/git-pincer/commit/1914ea0470552702845f5377cfeb6e55df122b97)) - Zero
- **(ui)** flicker-free menu execution and polished main-menu layout - ([eae2df7](https://github.com/zlx2019/git-pincer/commit/eae2df7b5835b9b126f50302d797a32cf68675d5)) - Zero
- **(ui)** in-session success dialog and panel-style execution page - ([bf53aed](https://github.com/zlx2019/git-pincer/commit/bf53aed6077b2b5d1a859ea803a222071adb50fb)) - Zero
- **(ui)** RPG-style pixel main menu with repo vitals panel - ([c8ddb36](https://github.com/zlx2019/git-pincer/commit/c8ddb367fd269db54f0dd26e4781a410c931d8b1)) - Zero
- IDEA-style three-pane git conflict resolution TUI - ([e88d664](https://github.com/zlx2019/git-pincer/commit/e88d6648fd8cd5ba06f2bb9ed7bbec953f2d46c0)) - Zero
- cherry-pick / revert subcommands and transparent git output - ([502ca8b](https://github.com/zlx2019/git-pincer/commit/502ca8b792b27e8a7e58f65297317b5708ef546d)) - Zero
- interactive action menu for bare invocation - ([65c53b8](https://github.com/zlx2019/git-pincer/commit/65c53b856169601e094a47111a6700f274a4388e)) - Zero
- playground example generating a demo repo with every conflict scenario - ([fa9d1d7](https://github.com/zlx2019/git-pincer/commit/fa9d1d7fd5e5ac352d4381a35df82b42edcd1bdc)) - Zero

### Miscellaneous Chores

- **(release)** support pre-release tags in changelog and release marking - ([acfd80b](https://github.com/zlx2019/git-pincer/commit/acfd80b5a847de8a66ec5f5d0c6df11a8a2c7adf)) - Zero
- **(release)** production-grade build profile and packaging - ([14f3dfc](https://github.com/zlx2019/git-pincer/commit/14f3dfccc614ddb145f63bb0ebfa5e0b19608495)) - Zero
- update author email and tidy typos config - ([d0e8f81](https://github.com/zlx2019/git-pincer/commit/d0e8f81d299d92ac6f0cc1598bb1b9420867c7ac)) - Zero
- translate abort / file / run CLI messages to English - ([fb17e2d](https://github.com/zlx2019/git-pincer/commit/fb17e2db2156f96e993966a12268a6b1dd236b66)) - Zero
- translate CLI subcommand help to English - ([9394959](https://github.com/zlx2019/git-pincer/commit/939495962b15f00fd29afc597b192c4f733d30c9)) - Zero

### Other

- publish stable releases to crates.io - ([f82a134](https://github.com/zlx2019/git-pincer/commit/f82a134567c256d17b87088056c8f7418bbb0809)) - Zero

### Refactoring

- **(ui)** drop panel hard shadows from RPG menu - ([9165149](https://github.com/zlx2019/git-pincer/commit/91651492f85f33fcf8bf5f77ad44f9bf5a782aca)) - Zero
- rename project to git-pincer - ([1065ced](https://github.com/zlx2019/git-pincer/commit/1065ced5a133dd70d59f52606ec99ab3b94f545b)) - Zero

### Style

- **(ui)** red HP gauge and brighter empty gauge slots - ([152add3](https://github.com/zlx2019/git-pincer/commit/152add38b2ca33a30e134a204428a0fee57519ca)) - Zero

<!-- generated by git-cliff -->
