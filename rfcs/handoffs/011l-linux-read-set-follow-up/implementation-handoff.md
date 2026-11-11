# Handoff 011l — Linux read set, follow-up: other families' certificate bundles; `status` shows the dependency step's set (release 0.10)

**RFC.** [RFC 011](../../done/011-confinement-backends.md) (Amendment 1) · **Specification.** REQ-CONF-011, external design §13.3 · **From.** Handoff 011j's questions 1 and 2, decided 2026-11-11.

1. **The certificate bundles of the Fedora/RHEL and openSUSE families (011j question 1: option b).** Add `/etc/pki/tls/certs/ca-bundle.crt` and `/etc/ssl/ca-bundle.pem` to the dependency step's worker only, each admitted **only where it exists**. They are world-readable public data, and leaving them out would break the dependency step on those families, where 0.9.0 worked. The CHANGELOG and the host-tools page say plainly that these two are **unmeasured**: added from the families' documented layout, not from a failure seen. If the owner or anyone can run the step on such a machine, the request records it. The tests of 011j extend to them: present only in the step's set, never in the offline set.
2. **`status` names both read sets (011j question 2).** `status` shows `read_set` (the offline worker's, as now) and `dependency_step_read_set_extra` (the files the dependency step's worker adds), in text and JSON, so that a user can see what the networked step reads. Additive (`cli.md`, CHANGELOG). Test: both listed; the extra set holds exactly the step's files present on the machine.

Return: `.git-exclude/review-requests/011l-linux-read-set-follow-up/README.md`.
