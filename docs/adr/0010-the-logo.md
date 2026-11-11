# 0010. The logo

- Status: Accepted; amends [0002](0002-mpl-2-0.md) (the logo not under the MPL)
- Date: 2026-11-11

## Context

The owner chose Aerith's logo on 2026-11-11: a five-petal flower made of one folded ribbon, from
violet to cyan. It was made with an image generator. Its file is a raster image wrapped in an SVG,
and carried a signed provenance record (C2PA) in its metadata.

The owner's other projects keep their logo out of the code's licence, so that a fork cannot take it
(Magnolia's ADR-0012, uwui's in SlimeEngine's ADR-0183). An image made by a generator is weakly
protected by copyright; a mark is protected by its use as the program's sign, whatever made it.

## Decision

**The logo is not under the MPL-2.0.** It is Aerith's mark: it may be shown unchanged to refer to
Aerith; it may not be the logo of another project, a fork included, nor suggest that a project is
Aerith or endorsed by it. [`logo/README.md`](../../logo/README.md) says so beside the file.

**The repository's copy carries no metadata**: the provenance record was removed, the pixels kept
identical. The owner's original file is unchanged.

**It is shown centred at the head of the README**, as the owner's other projects do.

## Alternatives considered

- **The logo under the MPL with the code**: one licence for the whole repository, but any fork could
  take the logo.
- **Publishing the file with its metadata**: the provenance record would stay in the public history
  for ever.
- **Waiting for a vector drawing**: a cleaner file, but no logo until then.

## Consequences

- The repository holds one file the MPL does not cover; the README says so.
- The file is a raster image; its transparent background holds stray specks around the petals. A
  vector drawing of the same flower is an idea of the [PLAN](../PLAN.md#ideas).
