---
id: STD
---

# Standard helpers

Applies to the `std` module of BSL and to the conventions every helper in it follows.

## Terminology

### Helper

A function the runtime offers to scripts in the `std` module.

## The std module

The runtime MUST offer a module named `std` in the scope of every [script](overview.md#script), in the [static context](app.md#static-context), the [action context](app.md#action-context) and the [validator context](app.md#validator-context) alike.
A script calls a helper by its qualified name, as `std::name(arguments)`.
Calling a qualified name the module does not hold MUST raise a script error.
A function or variable a script defines MUST NOT change which helper a qualified `std::` call reaches.

> [!NOTE]
> Keeping every helper in one module means a helper added in a later Seedling version cannot collide with a function or variable an existing script already defines.

A helper's name MUST consist of lower-case ASCII letters, ASCII digits and underscores, and begin with a letter.

## Arguments

A helper MUST check its arguments as the [argument rules](overview.md#arguments) require.
A helper that parses text MUST raise a script error when the text is not of the form it parses, and MUST NOT return a partial or corrected result in its place.

Each parameter kind has a helper that parses a value of that kind, as [kind parsers](action.md#kind-parsers) specify.

## Results

A helper that parses compound text, such as a URL or an email address, MUST return an object map from the name of each part to its value.
A helper whose result is a single value with no parts MUST return it as a string, an integer or a boolean.
A helper MUST return a value of a type of its own only where that value needs methods, such as comparing two versions or testing whether a subnet holds an address.

## Replicability

A helper MUST return the same result, or raise the same script error, every time it is called with the same arguments.
A helper MUST NOT read the clock, the network, the host's filesystem, or any state of the app or host that its arguments do not carry.
A helper MUST NOT change the app, so every helper is callable in the validator context.

> [!NOTE]
> A helper is held to the [replicability](overview.md#replicability) of evaluation wherever it is called, including in the action context, so that a helper behaves the same whichever context a script calls it from.
