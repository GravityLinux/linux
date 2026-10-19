# Native object/mesh experiment on T8132

`asahi_m4.native_mesh=1` is an explicitly paired kernel/userspace experiment.
The default is **0 and must remain 0**. No capability bit or execution UAPI
is provided. The paired stream starts with the native object/mesh state and
currently supports only the T16 position-only, single-group resource class.

With the option enabled, **any render-node client can cause a runtime-wide
quarantine** by submitting a mesh launch whose operand admission never receives
a matching firmware reply. The ten-second admission timeout blocks all VMs
until the device is reset. CPU producer waits also reduce ordinary submission
pipelining. These availability costs are acceptable only for the opt-in
experiment; this option must never become a default or an advertised feature.

File close waits for its pending admission while retaining the VM and support
object. It polls event service with the runtime lock dropped between polls,
without consuming retirement receipts. A reply permits normal detach; timeout
or event-service failure retains the existing quarantine and mapping ownership.

Payload-register interpretation, capacity, concurrent use and the lifetime of
firmware registration across context-slot rebinds require device qualification.
The latter must be tested by returning to an admitted VM after its old slot is
assigned to another VM, then rendering normally in that new owner.
