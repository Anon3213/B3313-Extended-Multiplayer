# Notices

B3313 Extended Multiplayer is under the BSD Zero Clause License; see `LICENSE.txt`. It is not
affiliated with or endorsed by Nintendo or by the B3313 team. It contains no ROM: a legal copy
of Super Mario 64 and the B3313 patch are needed to use it.

Two things it is built from are other people's work, under their own terms.

## Troll Engine

The multiplayer ROM patch is built with Troll Engine, B3313's engine, which this repository
fetches (`scripts/engine.py`) and does not contain. `TrollEngine/Multiplayer/engine-hooks.patch`
quotes a few lines of it around each change, and the patch the panel carries holds engine
functions compiled again with those changes. Both are covered by Troll Engine's license:

> Copyright © 2022-2025 Chlorobyte
>
> Copyright © 2024 Emax64
>
> Redistribution and use in source and binary forms, with or without modification, are
> permitted provided that the following conditions are met:
>
> 1. Redistributions of source code must retain the above copyright notice, this list of
>    conditions and the following disclaimer.
>
> 2. Redistributions in binary form must reproduce the above copyright notice, this list of
>    conditions and the following disclaimer in the documentation and/or other materials
>    provided with the distribution.
>
> 3. Neither the name of the copyright holder nor the names of its contributors may be used to
>    endorse or promote products derived from this software without specific prior written
>    permission.
>
> THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS “AS IS” AND ANY EXPRESS
> OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF
> MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE
> COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL,
> EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE
> GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED
> AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING
> NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED
> OF THE POSSIBILITY OF SUCH DAMAGE.

Upstream lists contributions whose licensing it has not settled (its README, "Licensing").
None of the hooked files is among them.

## Rust crates and icons

The programs link crates under the MIT, Apache-2.0, BSD, ISC, Zlib, CC0 and Unicode licenses;
`deny.toml` and `panel/deny.toml` hold the lists the build enforces, and a release carries each
crate's license text in `THIRD-PARTY-LICENSES.txt` (see `scripts/release.py`). The panel's
icons are Lucide's, under the ISC license in `panel/assets/icons/LICENSE`.
