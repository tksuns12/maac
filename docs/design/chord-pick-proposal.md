# Picking chord members: proposal

**Status:** accepted 2026-10-15, when the owner took both recommended
[decisions](#decisions), and implemented; [Results](#results) records the
evidence. This is item 2 of
[what to build next](../reports/ai-production-trial-t5.md#what-to-build-next-in-order)
in the T5 trial: F31, an arpeggio that cannot follow the harmony. It is a
language change. It extends the
[harmony proposal](harmony-proposal.md#not-proposed), which deferred
"picking a middle member, such as a walking bass's fifth", and the
[strum and voicing proposal](strum-voicing-proposal.md#not-proposed), which
deferred picking by string.

## Why

"Afterlight" plays a plucked arpeggio over its four-bar loop, Fm–Db–Ab–Eb:
root, third, fifth, third, octave, fifth, third, fifth, in sixteenths. The
bass, the supersaw chords, the piano and the pad all follow one `harmony`.
The arpeggio cannot, because a chord leaf plays either all of its voicing,
its lowest `n` pitches (`bottom = n`) or its highest (`top = n`). Nothing
picks the second or the third. So the arpeggio is written per chord:

```maac
pattern arp_fm { length = 2q;
  note n1 { at = 0q; dur = 1/4q; pitch = F4; velocity = 0.7; }
  note n2 { at = 1/4q; dur = 1/4q; pitch = Ab4; velocity = 0.5; }
  // six more notes, then the same eight for Db, Ab and Eb
}
pattern arp_loop { length = 16q;
  use fm { pattern = &arp_fm; at = 0q; count = 2; }
  use db { pattern = &arp_db; at = 4q; count = 2; }
  use ab { pattern = &arp_ab; at = 8q; count = 2; }
  use eb { pattern = &arp_eb; at = 12q; count = 2; }
}
```

- **The progression is written again.** `arp_loop` restates the harmony's
  four chords and their positions, and each `arp_*` pattern spells its
  chord out. Together they are 46 lines and 32 notes.
- **A chord change misses the arpeggio.** Changing the harmony's third
  chord moves the bass, chords, piano and pad, but not the arpeggio, which
  goes on playing the old chord against them.
- **The pattern is one shape, written four times.** Every chord plays
  members 1, 2, 3, 2, 4, 3, 2, 3 of its voicing. The rhythm and the shape
  are the same; only the pitches differ, and those are what a voicing holds.

The same gap stopped a walking bass in the harmony proposal: a bass that
plays root, third, fifth and octave of each chord needs the third and the
fifth.

## The existing position, and why this proposal extends it

`top` and `bottom` were added for strums: an upstroke catches the upper
strings, and a piano's left hand plays the root while the right plays the
rest. Both select a run of pitches from one end. An arpeggio and a walking
bass select single pitches from the middle. The voicing already orders its
pitches, so a position in it is enough to name one: the second pitch of
`[F4, Ab4, C5, F5]` is Ab4.

The [design principles](design-principles.md) hold. A position is finite,
explicit data. Nothing is generated: the arpeggio's order and rhythm stay
written in the pattern, one chord leaf per note, and the voicing stays the
arranger's choice.

## Proposed contract

A chord leaf may take `pick`, one position or a list of positions in the
pitches it would otherwise play: its `pitches`, its `voicing`, or the
voicing a `use` or `place` supplies.

```maac
voicing arp_fm { pitches = [F4, Ab4, C5, F5]; }
pattern arp_cell { length = 2q;
  chord n1 { at = 0q; dur = 1/4q; pick = 1; velocity = 0.7; }
  chord n2 { at = 1/4q; dur = 1/4q; pick = 2; velocity = 0.5; }
  chord n3 { at = 1/2q; dur = 1/4q; pick = 3; velocity = 0.58; }
  // ...
}
```

- **Positions.** Positions count from 1, the first pitch in the list, as
  `bottom = 1` plays the first. A list gives several, strictly increasing:
  `pick = [1, 3]` plays the first and third. Each must be an integer from 1
  through the number of pitches; otherwise it is `E_RANGE`.
- **Played pitches.** The picked pitches, in list order, are the chord's
  played pitches. Member addresses, per-member lists (`velocity`,
  `onset_offset` and the rest), strums and expressions then follow §8.2's
  existing rules: `n2` above plays one member, `n2.0`.
- **One selection.** `pick` beside `top` or `bottom` is `E_RANGE`.
- **Supplied voicings.** As for `top` and `bottom`, a position is checked at
  each occurrence, and one beyond a supplied voicing is `E_RANGE`, naming the
  chord and the use or placement that supplied the voicing. A harmony whose
  voicings differ in size therefore needs picks that every one of them has.
- **Editing.** §21's rule for shifted members covers `pick`: an edit that
  changes the played pitches through `pick` shifts later members, and an
  existing override that addresses a shifted member fails with
  `E_INSTANCE_TARGET`.
- **Lowering.** A picked chord lowers to its members like any chord, so the
  plan format, the renderer and every reader of plans are unchanged.

## Effect on T5

"Afterlight"'s arpeggio becomes four voicings, a `voicings` map and one
pattern of eight picked chords, placed with the shared harmony:

```maac
voicing arp_fm { pitches = [F4, Ab4, C5, F5]; }
voicing arp_db { pitches = [F4, Ab4, Db5, F5]; }
voicing arp_ab { pitches = [Eb4, Ab4, C5, Eb5]; }
voicing arp_eb { pitches = [Eb4, G4, Bb4, Eb5]; }
voicings arp_voicings { chords = [("Fm", &arp_fm), ("Db", &arp_db), ("Ab", &arp_ab), ("Eb", &arp_eb)]; }
place arp_intro { pattern = &arp_cell; track = &arps; at = 0q; count = 16; harmony = &loop_chords; voicings = &arp_voicings; }
```

The 46 lines become 15, the progression is written once for all five
parts, and the notes are the same pitches at the same times and velocities.

## Rejected alternatives

- **An arpeggiator**, such as `arp = up; rate = 1/4q;` on a chord, which
  generates the notes. The order, rhythm and accents of an arpeggio are the
  part itself; a generator would hide them behind a rule, and the
  principles keep general computation in tools. A tool can still write the
  picked chords.
- **Positions from zero**, matching member addresses such as `n2.0`. `top`
  and `bottom` count from 1, and so do musicians: the first, the third.
  Addresses index the played pitches, not the voicing, so they differ from
  positions anyway.
- **Picks in any order**, such as `pick = [3, 1]` to strike the third first.
  The order of the played pitches would then differ from the voicing's,
  which every per-member list and strum depends on. A different order is a
  different voicing or a strum `up`.
- **Wrapping or clamping a position beyond the voicing.** A triad asked for
  its fourth pitch could wrap to the root or clamp to the top. Either would
  play a pitch the arranger did not write, silently. An error says which
  chord and which voicing disagree.
- **Melody notes relative to the chord**, such as scale degrees. Broader,
  and still not needed: an arpeggio picks chord tones, which the voicing
  holds.

## Not proposed

- **Positions from the top**, such as `pick = -2` for the second-highest.
  `top = 1` covers the highest; nothing has needed the others.
- **Passing notes in a walking bass.** A chromatic approach note is not in
  the chord; it stays a note with its own pitch.
- **Picking by string with muted strings**, the guitar model the strum
  proposal deferred. Positions in a voicing are enough for arpeggios and
  basses; a string model waits until a part needs it.

## Versioning

An additive field in `maac 1`, as the strum, voicing and harmony fields
were: older readers reject `pick` as an unknown field, and nothing else
changes. The specification's §8.2 and §21 gain the rules, and the
conformance index a vector. The changelog lists it under "Added".

## Acceptance

1. `pick` with one position and with a list plays exactly those pitches, in
   order, from `pitches`, from a `voicing` and from a supplied voicing.
2. Per-member lists, strums and expressions apply to the picked members as
   to any played pitches; addresses are `chord.k` over the picked members.
3. `E_RANGE` for a position of 0, a non-integer, a list that does not
   strictly increase, a position beyond the pitches, and `pick` beside `top`
   or `bottom`; the error for a supplied voicing names the supplier.
4. Under a harmony, a picked chord plays the chord sounding at its position,
   with the same lookup as any chord.
5. Editing: changing `pick` shifts later members, and an override on a
   shifted member is refused, as for `top` and `bottom`.
6. Semantic validation, compilation and the editing kernel agree on the
   played pitches; the normalization agreement tests cover a picked chord.
7. "Afterlight"'s arpeggio is rewritten with `pick`; its events have the
   same pitches, times and velocities as before.

## Cost and risk

- **Three code paths.** The selection is implemented in semantic
  validation, the compiler and the editing kernel, as `top` and `bottom`
  are; each gains a third case. The selection is no longer one contiguous
  run, so the code returns the picked pitches rather than a slice.
- **Same-size voicings.** A part whose voicings differ in size must pick
  positions they all have. The error names the chord and the supplier, so
  this is found at `maac check`, not by ear.

## Decisions

1. **The field.** `pick`, a position or a strictly increasing list,
   counted from 1 (recommended); or `member = k` for one position only.
2. **Beyond the voicing.** An error (recommended), or wrap around.

The owner chose `pick` and an error for a position beyond the voicing.

## Results

- **Contract.** Semantic validation, the compiler and the editing kernel
  each select the picked pitches, and §8.2 states the rule, with an
  arpeggio placed under a harmony as its example; §21 names `pick` among
  the fields whose edits shift members. The positions are checked as
  proposed. A position that is not a number is `E_UNIT`, as for `top` and
  `bottom`. With `pick`, a chord's number of members is known before any
  voicing is supplied, so `maac check` compares per-member lists with it
  directly. A strum counts only the picked members: the conformance
  arithmetic gains `pick_strum_up_member0_velocity`, an upstroke over
  `pick = [2, 4]`.
- **One spelling decision.** `pick = 2` means `pick = [2]`, so both
  normalizers write a single position as a list of one, and the two
  spellings have one execution identity. The normalization agreement test
  covers a picked chord, and changing a picked position changes the hash.
- **Tests.** `tests/chord_pick.rs` checks acceptance items 1 to 5: picked
  chords play like their written pitches under transposition, stretch and
  `cut`, with per-member lists, a strum and a pitch expression; a pick
  follows a supplied voicing and each chord of a harmony; every malformed
  pick is refused, and the error for a supplied voicing names the use; an
  edit that drops or inserts a picked member before an override is
  `E_INSTANCE_TARGET`, while a pick changed in place or extended on top is
  not; and materializing a placement keeps its picked members.
- **"Afterlight".** The arpeggio is four voicings, a `voicings` map and one
  cell of eight picked chords, placed seven times with the shared harmony:
  46 lines become 15. All 3,589 events of the track are identical in order,
  pitch, time, velocity and every other field except their addresses and
  sources. The rendered master is not bit-identical: an instrument sums its
  voices in address order, and the new addresses change that order where
  the pluck's release overlaps the next note. 793 of 15,744,000 samples
  differ, by at most 6×10⁻⁸ (−144.5 dBFS), half of a 24-bit step. The
  README's recording is unchanged.
