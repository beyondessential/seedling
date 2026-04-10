# Resource Context Rules

Resources in BSL are either **static** or **dynamic** depending on where they are defined:

- **Static** resources are defined at the top level of the script, outside any action
  closure. They represent the app's desired steady state and are managed by the
  reconciler between operations.
- **Dynamic** resources are defined inside action closures. They exist only for the
  duration of the operation and are cleaned up automatically when the action ends.

## Named vs anonymous

- **Named** resources in the static context are created and registered.
- **Named** resources in the dynamic context are **references** to existing static
  resources. If no static resource with that name exists, it is an error.
- **Anonymous** resources (no name argument) can only be created in the dynamic context.
  Attempting to create an anonymous resource in the static context is an error.

## Resource support matrix

| Resource | Named, static | Named, dynamic | Anonymous, static | Anonymous, dynamic |
|---|---|---|---|---|
| `Deployment` | ✓ create | ✓ reference | ✗ error | ✓ create |
| `Job` | ✓ create | ✓ reference | ✗ error | ✓ create |
| `Service` | ✓ create | ✓ reference | ✗ error | ✓ create |
| `HttpService` | ✓ create | ✓ reference | ✗ error | ✓ create |
| `Volume` | ✓ create | ✓ reference | ✗ error | ✓ create |
| `Ingress` | ✓ create | ✓ reference | ✗ error | ✗ error |
| `ExternalService` | ✓ reference | ✓ reference | ✗ error | ✗ error |
| `ExternalVolume` | ✓ reference | ✓ reference | ✗ error | ✗ error |

### Notes

- **Services are allowed as anonymous dynamic resources** because they serve as the
  internal networking primitive connecting dynamic deployments and jobs together within
  an operation. For example, an action might start an anonymous deployment alongside an
  anonymous job that communicates with it over an anonymous service.

- **Ingress has no anonymous form** in any context. An ingress is always an explicitly
  configured, externally-visible routing rule and has no meaningful transient equivalent.

- **ExternalService and ExternalVolume are always references.** They point to resources
  that Seedling does not own or manage, so creation does not apply.

## Volume lifecycle

The named/anonymous distinction also governs volume lifecycle:

- **Named static volumes** are created once by the reconciler and persist across
  container restarts. Writes (`volume.write(...)`) are applied only at creation time.
- **Anonymous dynamic volumes** are created by the actuator immediately before the
  container starts and removed when the container stops. Writes are applied on each
  creation.

Named volumes defined inside an action closure (dynamic named volumes) are created by
the actuator if they do not already exist, and are never automatically removed — they
have user-controlled lifetime.