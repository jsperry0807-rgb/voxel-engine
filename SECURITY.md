# Security Policy

## Supported versions

The engine is pre-1.0 and has no released versions. Only `main` receives fixes;
tagged milestones (such as `m3`) are historical snapshots and are not patched.

## Reporting a vulnerability

Report privately through this repository's **Security** tab, using
[Report a vulnerability](https://github.com/jsperry0807-rgb/voxel-engine/security/advisories/new).
Do not open a public issue, and do not post the details in a discussion, commit
or pull request.

Please include:

- what the flaw lets someone do, and who it affects;
- the crate involved, and where relevant the chunk edge (`64³` or `128³`) and
  the feature flags you built with;
- the commit you tested (`git rev-parse --short HEAD`), your platform, and the
  wgpu backend;
- the smallest reproduction you have — a command, a seed, or a crafted input;
- whether the entry point is a save file, a world seed, a network message, a
  shader or an asset pipeline.

There is no response-time guarantee: the project has a single maintainer and is
worked on part-time. You will get an acknowledgement, an assessment of whether
the report is in scope, and a credit line in the advisory unless you ask to
stay anonymous.

## Scope

In scope: memory-safety, denial-of-service, arbitrary file access, shader or
asset pipeline flaws, and anything that lets a world, save or asset escape the
sandbox the engine assumes for it.

Out of scope: crashes with no security impact (open a normal issue instead),
vulnerabilities in third-party dependencies (report them upstream, then tell us
so the pin can move), and anything requiring a machine you already control.

## Disclosure

The advisory stays private until a fix is on `main`. If the report is accepted,
the fix, the advisory and the credit are published together; if it is declined,
you are told why and may publish on your own schedule.
