# Packaging

Topos ships in six ways, all built from the same release:

| Channel | What users get | Built by | Published by |
|---|---|---|---|
| GitHub release | `Topos-v<version>-x86_64-windows.zip`, `...-x86-windows.zip` | `release.yml` | you, by publishing the draft |
| Installer | `Topos-v<version>-x86_64-setup.exe`, `...-x86-setup.exe` | `release.yml` (Inno Setup) | you, by publishing the draft |
| winget | `winget install TUIGSE.Topos` | `render.py` | `packages.yml` |
| Chocolatey | `choco install topos` | `render.py` | `packages.yml` |
| Scoop | `scoop install topos` | `render.py` | the `Tuigse-oss/scoop-topos` bucket |
| Microsoft Store | Topos in the Store | `release.yml` (`msix/build-msix.ps1`) | `packages.yml` |

This folder holds the templates for winget, Chocolatey, and Scoop, which
`render.py` fills in with the version, download URLs, and checksums from a
release's `SHA256SUMS.txt`, and the MSIX manifest template and build script
for the Store. When a GitHub release is published, `.github/workflows/packages.yml`
publishes to winget, Chocolatey, and the Store, and the Scoop bucket updates
itself.

To render the package-manager manifests by hand, for a first submission or
to check a change:

```bash
gh release download v0.1.0 --repo Tuigse-oss/Topos --pattern SHA256SUMS.txt
python3 packaging/render.py 0.1.0 SHA256SUMS.txt out
```

## Cutting a release

Set `version` in `Cargo.toml` (plain X.Y.Z), merge, and push a matching tag
(`v0.1.0`). The release workflow runs a RustSec audit, then builds and
tests both architectures (zip, installer, and MSIX package), bundles the
Store package, writes `SHA256SUMS.txt`, attests every file, and attaches
everything to one draft release. Publishing the draft triggers winget,
Chocolatey, and the Store, each once it is switched on (below). Running the release workflow by hand builds the
same files as a workflow artifact, without a release.

By hand, on Windows with Inno Setup 6.6 or later and the Windows SDK:

```powershell
cargo build --release --target x86_64-pc-windows-msvc
cargo run --release --example art -- installer-art
# package-x64\ holds Topos.exe, the licenses, the notices, and README.md
iscc /DAppVersion=0.1.0 /DTargetArch=x64 installer\topos.iss
packaging\msix\build-msix.ps1 -Version 0.1.0 -Arch x64 -SourceDir package-x64 `
  -ArtDir installer-art -OutDir msix
installer\test-installer.ps1 -Arch x64 -Old dist\Topos-v0.1.0-x86_64-setup.exe -OldVersion 0.1.0 `
  -New dist\Topos-v0.1.0-x86_64-setup.exe -NewVersion 0.1.0 -ProgramVersion 0.1.0
installer\test-msix.ps1 -Package msix\Topos-v0.1.0-x64.msix -Version 0.1.0
```

The installer test installs for the current user and for all users (it
needs an administrator prompt), upgrades while Topos is running, and
uninstalls. The MSIX test trusts a throwaway certificate, installs a signed
copy, runs it through its `topos` alias, and removes both. Run them on a
test machine or VM, not your everyday one.

## What a release contains

Each release has these files, all listed in one `SHA256SUMS.txt`:

- `Topos-v<version>-x86_64-windows.zip` and `Topos-v<version>-x86-windows.zip`:
  `Topos.exe`, the MIT and Apache-2.0 license files,
  `THIRD-PARTY-NOTICES.txt`, and the README. Unzip and run; nothing is
  installed.
- `Topos-v<version>-x86_64-setup.exe` and `Topos-v<version>-x86-setup.exe`:
  the Inno Setup installers.
- `Topos-v<version>-store.msixupload`: the Microsoft Store package (an MSIX
  bundle holding both architectures, wrapped for Partner Center). It is not
  meant to be installed by hand: it is unsigned, because the Store signs
  what it distributes. Releases made before the Store identity is set
  (below) leave it out.

None of the files is code-signed (see the README's Install section).

## Upgrades

Every channel upgrades by installing the new version over the old one:

- **winget and Chocolatey** run the new Inno Setup installer silently. Its
  fixed `AppId` makes it replace the existing install in place, in the same
  folder and the same mode (per user or all users). winget finds the install
  through `ProductCode`, the Add/Remove Programs key that `AppId` produces;
  both architectures share it.
- A running Topos is asked to close first (Restart Manager). It is never
  restarted afterwards, so it cannot come back with the installer's
  administrator rights.
- **Scoop** unpacks each version into its own folder and switches `current`
  over to it.
- **The Store** updates the package itself.
- Settings (`settings.txt`) and each drive's last scan (`*.toposcan`) live in
  `%LOCALAPPDATA%\Topos`, never in the install folder, so upgrades and
  uninstalls of the zip, installer, winget, Chocolatey, and Scoop builds
  keep them. The Store build is different: Windows redirects its
  `%LOCALAPPDATA%` writes into the package's own data folder, which is
  removed with the package. The two builds therefore do not share settings.
- Setup writes no registry settings and no machine-wide defaults; the
  appearance and every other choice is a per-user setting in the program.

CI checks this on every pull request, for both architectures:

- `installer\test-installer.ps1`: a per-user and an all-users install, an
  upgrade of each while Topos is running (Setup must close it and must not
  restart it), a reinstall, an uninstall, the Add/Remove Programs entry,
  and that `%LOCALAPPDATA%\Topos` survives. A release build upgrades from
  the last published release's installer instead.
- `installer\test-msix.ps1`: the MSIX package is signed (a copy, with a
  throwaway certificate), installed, run through its `topos.exe` alias,
  and removed. A release build also checks the bundle that goes to the
  Store.

There is no Chocolatey parameter test: the Topos package takes no
parameters.

Never change the `AppId` in `installer/topos.iss`: a new one would install
next to the old version instead of replacing it. For the same reason never
change the Store package's identity name once it is in Partner Center.

## Scoop

The bucket is its own repository, `Tuigse-oss/scoop-topos`, so `main` here
never needs a bot commit. It does not exist yet. To set it up, give it an
`update` workflow that runs every six hours (and on demand), finds the latest published Topos release, renders
`scoop/topos.json` with this folder's `render.py` at that tag, and commits it
as `bucket/topos.json`. It uses that repository's own token, so no secret is
needed.

GitHub pauses scheduled workflows in a public repository after 60 days without
activity; if a release is not picked up, re-enable or run the `update`
workflow there.

Users install with:

```powershell
scoop bucket add topos https://github.com/Tuigse-oss/scoop-topos
scoop install topos
```

Scoop picks the 64-bit zip on 64-bit Windows and the 32-bit one otherwise.

## Switching on winget and Chocolatey

Each needs a repository variable (Settings > Secrets and variables > Actions >
Variables) and a secret. Without the variable, its job is skipped.

### winget: `PUBLISH_WINGET=true`, secret `WINGET_TOKEN`

1. Fork `microsoft/winget-pkgs` to the account that will open the pull
   requests.
2. Create a classic personal access token on that account with the
   `public_repo` scope and add it as the secret `WINGET_TOKEN`.
3. A new package is reviewed by hand, so submit the first version yourself:
   render the manifests as above, then on Windows run
   `wingetcreate submit out\winget --token <token>`. Turn the variable on once
   that pull request is merged; later versions are submitted by the workflow.

Users install with `winget install TUIGSE.Topos` (add `--scope machine` for
all users). winget picks the installer for the machine's architecture.

### Chocolatey: `PUBLISH_CHOCOLATEY=true`, secret `CHOCOLATEY_API_KEY`

1. Create an account at https://community.chocolatey.org and copy the API key
   from the account page into the secret `CHOCOLATEY_API_KEY`.
2. Every version is checked by Chocolatey's moderators before it is listed;
   the first one is reviewed by a person and can take a while.

Users install with `choco install topos` (from an administrator prompt;
Chocolatey installs for all users). The package takes no parameters.

## Microsoft Store

The release workflow builds an MSIX package per architecture from
`msix/AppxManifest.xml` with `msix/build-msix.ps1` (layout, `makepri` for the
scaled logos, `makeappx pack`), tests each, bundles them into one
`.msixbundle`, tests that, and wraps it as `Topos-v<version>-store.msixupload`.
The package is a full-trust desktop app (`runFullTrust`) with the alias
`topos.exe`, for Windows 10 1809 and later. It is uploaded unsigned; the Store
signs it.

The package identity (name and publisher) must be exactly the one Partner
Center assigns. Until the repository variables below are set, test builds
(pull requests and manual runs) use clearly test-only values
(`TUIGSE.Topos.TestOnly`, `CN=TUIGSE Topos Test Only`) and warn: they
install and test fine, but the Store rejects them. A tagged release made
before the variables are set still builds and tests the Store package but
leaves it out of the release, with a warning, so a test-only package is
never published; and the `msstore` job refuses to upload one.

### One-time setup

1. **Developer account.** In Partner Center
   (https://storedeveloper.microsoft.com), register a **company** developer
   account for Tuigse LLC. Registration is free (Microsoft dropped the
   company fee in 2026; check the amount shown at sign-up). Microsoft
   verifies the company, which can take a few days.
2. **Reserve the name.** Apps and games > New product > MSIX or PWA app,
   and reserve the name `Topos`.
3. **Copy the identity into the repository.** In the new product, open
   Product management > Product identity and add these repository variables
   (Settings > Secrets and variables > Actions > Variables), copying each
   value exactly:

   | Partner Center (Product identity) | Repository variable |
   |---|---|
   | `Package/Identity/Name` | `MSIX_IDENTITY_NAME` |
   | `Package/Identity/Publisher` | `MSIX_PUBLISHER` |
   | `Package/Properties/PublisherDisplayName` | `MSIX_PUBLISHER_DISPLAY_NAME` |
   | Store ID (such as `9NXXXXXXXXXX`) | `MSSTORE_PRODUCT_ID` |

   Then cut a release (or run the release workflow by hand for a test build)
   so the `.msixupload` carries the real identity. Check the build log: the
   "MSIX identity not set" warning must be gone.
4. **First submission, by hand.** The msstore CLI can only update an app
   that is already in the Store. In Partner Center, start a submission and
   fill in:
   - **Packages:** upload `Topos-v<version>-store.msixupload` from the
     release.
   - **Restricted capabilities:** Partner Center asks why the package
     declares `runFullTrust`. Answer: "Win32 desktop app packaged as MSIX."
   - **Privacy policy:** Topos collects nothing: no telemetry, no
     analytics, no network access, no account. Partner Center may still ask
     for a privacy policy URL; if it does, use
     <https://github.com/Tuigse-oss/Topos/blob/main/PRIVACY.md>.
   - **Pricing:** free. (Automated updates only work for free products.)
   - The store listing (description, screenshots), the age rating
     questionnaire, and properties (category: Utilities & tools).

   Submit it for certification and wait until Topos is live in the Store.
5. **Automated updates.** So that `packages.yml` can submit later versions:
   1. Partner Center > Account settings > Tenants: if the account has no
      Microsoft Entra ID tenant yet, associate one (or create one there).
   2. In the Entra admin center (https://entra.microsoft.com), App
      registrations > New registration, for example "Topos Store
      publishing". Under Certificates & secrets, create a client secret and
      copy its value at once (it is shown only once).
   3. Partner Center > Account settings > User management > Microsoft Entra
      applications: add that application with the **Manager** role.
   4. Add these repository **secrets**:

      | Secret | Where it comes from |
      |---|---|
      | `AZURE_AD_TENANT_ID` | Entra admin center > Overview > Tenant ID |
      | `AZURE_AD_APPLICATION_CLIENT_ID` | the app registration's Application (client) ID |
      | `AZURE_AD_APPLICATION_SECRET` | the client secret's value |
      | `SELLER_ID` | Partner Center > Account settings: the Seller ID |

   5. Set the repository variable `PUBLISH_MSSTORE=true`.

   Client secrets expire (at most two years); put a reminder in the
   calendar to replace `AZURE_AD_APPLICATION_SECRET` before then.

From then on, publishing a GitHub release runs the `msstore` job: it checks
the `.msixupload` against `SHA256SUMS.txt` and the identity variables,
installs Microsoft's Store Developer CLI (pinned by version and hash), and
runs `msstore publish`, which creates a submission from the last published
one (listing, pricing, and age rating carry over), uploads the package, and
commits it. Certification then takes from a few hours to a few days before
the update is live. A failed upload can be retried from the Actions tab
(packages workflow, Run workflow, with the tag).
