-- Issue #41: a D-STAR channel needs its three call signs, and two of them
-- cannot be worked out from anything already in the row.
--
-- Until now both D-STAR drivers DERIVED all three. `Your Callsign` was always
-- the literal CQCQCQ, and RPT1/RPT2 were built from `channels.callsign` plus a
-- band-to-port rule (>=1200 -> A, >=400 -> B, else C) with the gateway as the
-- same call plus G.
--
-- That rule holds up against a real radio — all five D-STAR memories in a
-- TH-D75 save carry exactly what it would have produced (`W0QEY  B`,
-- `KC0DS  C`, `WW8L   B`) — but it can only ever produce one answer per
-- frequency, and D-STAR has three things it cannot express:
--
--   * `Your Callsign` is not always CQCQCQ. Reflector linking (`REF030CL`),
--     unlinking, and call-sign routing (`/W0QEYB`) all live in that field, and
--     none of them is derivable from a repeater's frequency.
--   * The port letter is a property of the REPEATER, not of the band. The
--     convention is near-universal and the rule follows it, but a repeater that
--     departs from it had no way to say so.
--   * A personal hotspot's RPT1 is the OWNER's call sign, not a repeater's.
--
-- Nullable, and NULL keeps the derivation. That matters: the channel library
-- here holds 700+ D-STAR repeaters imported from RepeaterBook, which carries a
-- call sign but no module letter, so requiring these fields would blank the
-- RPT1 of every one of them. A stored value is an override, not a replacement
-- ([[channels-are-radio-agnostic]] — these are facts about the repeater, so
-- every D-STAR radio reads the same three columns).

ALTER TABLE channels ADD COLUMN dstar_ur_call TEXT;
ALTER TABLE channels ADD COLUMN dstar_rpt1    TEXT;
ALTER TABLE channels ADD COLUMN dstar_rpt2    TEXT;
