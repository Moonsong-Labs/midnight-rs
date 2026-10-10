(analyzed-ir (compiler-version "0.35.103-dev")
 (language-version "0.27.0") (runtime-version "0.20.101")
 (exports (casts . %casts.33) (curve_neg . %curve_neg.34)
   (curve_ops . %curve_ops.31) (field_arith . %field_arith.32)
   (field_reduce . %field_reduce.29) (hits . %hits.30)
   (ledger_ops . %ledger_ops.27)
   (persistent_hashes . %persistent_hashes.28)
   (scores . %scores.25) (scratch . %scratch.26)
   (seen . %seen.23) (tag_cell . %tag_cell.24)
   (transient_conversions . %transient_conversions.21)
   (transient_hashes . %transient_hashes.22)
   (uint_hash . %uint_hash.20))
 (contract-types)
 (kernel-declaration (%kernel.87 () (exported #f) (Kernel)))
 (public-ledger-declaration
   (public-ledger-array
     (%scratch.26
       (0)
       (exported #t)
       (__compact_Cell (tfield (field-native))))
     (%tag_cell.24
       (1)
       (exported #t)
       (__compact_Cell (tbytes 32)))
     (%seen.23 (2) (exported #t) (Set (tbytes 32)))
     (%scores.25
       (3)
       (exported #t)
       (Map (tfield (field-native))
            (tunsigned 18446744073709551615)))
     (%hits.30 (4) (exported #t) (Counter)))
   (constructor () (tuple)))
 (native %transientHash.70
   (entry "__compactRuntime.transientHash" circuit)
   (type-arguments (tvector 2 (tfield (field-native))))
   ((%value.88 (tvector 2 (tfield (field-native)))))
   (tfield (field-native)))
 (native %transientCommit.73
   (entry "__compactRuntime.transientCommit" circuit)
   (type-arguments (tfield (field-native)))
   ((%value.89 (tfield (field-native)))
     (%rand.90 (tfield (field-native))))
   (tfield (field-native)))
 (native %persistentHash.76
   (entry "__compactRuntime.persistentHash" circuit)
   (type-arguments (tvector 2 (tbytes 32)))
   ((%value.91 (tvector 2 (tbytes 32)))) (tbytes 32))
 (native %persistentHash.60
   (entry "__compactRuntime.persistentHash" circuit)
   (type-arguments (tunsigned 18446744073709551615))
   ((%value.92 (tunsigned 18446744073709551615))) (tbytes 32))
 (native %persistentCommit.78
   (entry "__compactRuntime.persistentCommit" circuit)
   (type-arguments (tbytes 32))
   ((%value.93 (tbytes 32)) (%rand.94 (tbytes 32)))
   (tbytes 32))
 (native %degradeToTransient.63
   (entry "__compactRuntime.degradeToTransient" circuit)
   (type-arguments) ((%x.95 (tbytes 32)))
   (tfield (field-native)))
 (native %upgradeFromTransient.65
   (entry "__compactRuntime.upgradeFromTransient" circuit)
   (type-arguments) ((%x.96 (tfield (field-native))))
   (tbytes 32))
 (native %jubjubPointX.52
   (entry "__compactRuntime.jubjubPointX" circuit)
   (type-arguments) ((%pt.97 (tpoint (curve-jubjub))))
   (tfield (field-native)))
 (native %jubjubPointY.54
   (entry "__compactRuntime.jubjubPointY" circuit)
   (type-arguments) ((%pt.98 (tpoint (curve-jubjub))))
   (tfield (field-native)))
 (native %ecAdd.47 (entry "__compactRuntime.ecAdd" circuit)
   (type-arguments)
   ((%a.99 (tpoint (curve-jubjub)))
     (%b.100 (tpoint (curve-jubjub))))
   (tpoint (curve-jubjub)))
 (native %ecNeg.56 (entry "__compactRuntime.ecNeg" circuit)
   (type-arguments) ((%a.101 (tpoint (curve-jubjub))))
   (tpoint (curve-jubjub)))
 (native %ecMul.50 (entry "__compactRuntime.ecMul" circuit)
   (type-arguments)
   ((%a.102 (tpoint (curve-jubjub)))
     (%b.103 (tfield (field-scalar (curve-jubjub)))))
   (tpoint (curve-jubjub)))
 (native %ecMulGenerator.43
   (entry "__compactRuntime.ecMulGenerator" circuit)
   (type-arguments)
   ((%b.104 (tfield (field-scalar (curve-jubjub)))))
   (tpoint (curve-jubjub)))
 (native %hashToCurve.45
   (entry "__compactRuntime.hashToCurve" circuit)
   (type-arguments (tvector 2 (tfield (field-native))))
   ((%value.105 (tvector 2 (tfield (field-native)))))
   (tpoint (curve-jubjub)))
 (circuit %field_arith.32 (exported #t) (pure #f) (proof #t)
   ((%a.81 (tfield (field-native)))
     (%b.82 (tfield (field-native))))
   (tfield (field-native))
   (let* (((%p.15 (tfield (field-native))) (* (tfield
                                                (field-native))
                                              (var-ref %a.81)
                                              (var-ref %b.82))))
     (let* (((%s.16 (tfield (field-native))) (+ (tfield
                                                  (field-native))
                                                (var-ref %p.15)
                                                (var-ref %a.81))))
       (let* (((%d.83 (tfield (field-native))) (- (tfield
                                                    (field-native))
                                                  (var-ref %s.16)
                                                  (var-ref %b.82))))
         (seq (public-ledger %scratch.26 write (0) write (ttuple)
                (instructions
                  (push
                    (storage #f)
                    (value (state-value cell (align 0 1))))
                  (push
                    (storage #t)
                    (value (state-value cell (var-ref %d.83))))
                  (ins (cached #f) (n 1)))
                (var-ref %d.83))
              (return (var-ref %d.83)))))))
 (circuit %field_reduce.29 (exported #t) (pure #f) (proof #t)
   ((%c.84 (tfield (field-native)))
     (%q.85 (tfield (field-native))))
   (tfield (field-native))
   (let* (((%r.86 (tfield (field-native))) (- (tfield
                                                (field-native))
                                              (var-ref %c.84)
                                              (* (tfield (field-native))
                                                 (var-ref %q.85)
                                                 '6554484396890773809930967563523245729705921265872317281365359162392183254199))))
     (seq (public-ledger %scratch.26 write (0) write (ttuple)
            (instructions
              (push (storage #f) (value (state-value cell (align 0 1))))
              (push
                (storage #t)
                (value (state-value cell (var-ref %r.86))))
              (ins (cached #f) (n 1)))
            (var-ref %r.86))
          (return (var-ref %r.86)))))
 (circuit %transient_hashes.22 (exported #t) (pure #f) (proof #t)
   ((%x.71 (tfield (field-native)))
     (%r.72 (tfield (field-native))))
   (tfield (field-native))
   (let* (((%h.74 (tfield (field-native))) (call
                                             %transientHash.70
                                             (tuple
                                               (single (var-ref %x.71))
                                               (single (var-ref %r.72))))))
     (let* (((%c.75 (tfield (field-native))) (call
                                               %transientCommit.73
                                               (var-ref %h.74)
                                               (var-ref %r.72))))
       (seq (public-ledger %scratch.26 write (0) write (ttuple)
              (instructions
                (push (storage #f) (value (state-value cell (align 0 1))))
                (push
                  (storage #t)
                  (value (state-value cell (var-ref %c.75))))
                (ins (cached #f) (n 1)))
              (var-ref %c.75))
            (return (var-ref %c.75))))))
 (circuit %persistent_hashes.28 (exported #t) (pure #f) (proof #t)
   ((%x.77 (tbytes 32))) (tbytes 32)
   (let* (((%h.79 (tbytes 32)) (call
                                 %persistentHash.76
                                 (tuple
                                   (single
                                     '#vu8(111 112 115 58 112 104 58 0 0 0
                                           0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0
                                           0 0 0 0 0 0))
                                   (single (var-ref %x.77))))))
     (let* (((%c.80 (tbytes 32)) (call
                                   %persistentCommit.78
                                   (var-ref %h.79)
                                   (var-ref %x.77))))
       (seq (public-ledger %tag_cell.24 write (1) write (ttuple)
              (instructions
                (push (storage #f) (value (state-value cell (align 1 1))))
                (push
                  (storage #t)
                  (value (state-value cell (var-ref %c.80))))
                (ins (cached #f) (n 1)))
              (var-ref %c.80))
            (return (var-ref %c.80))))))
 (circuit %uint_hash.20 (exported #t) (pure #f) (proof #t)
   ((%n.61 (tunsigned 18446744073709551615))) (tbytes 32)
   (let* (((%h.62 (tbytes 32)) (call
                                 %persistentHash.60
                                 (var-ref %n.61))))
     (seq (public-ledger %tag_cell.24 write (1) write (ttuple)
            (instructions
              (push (storage #f) (value (state-value cell (align 1 1))))
              (push
                (storage #t)
                (value (state-value cell (var-ref %h.62))))
              (ins (cached #f) (n 1)))
            (var-ref %h.62))
          (return (var-ref %h.62)))))
 (circuit %transient_conversions.21 (exported #t) (pure #f)
   (proof #t) ((%x.64 (tbytes 32))) (tfield (field-native))
   (let* (((%f.66 (tfield (field-native))) (call
                                             %degradeToTransient.63
                                             (var-ref %x.64))))
     (let* (((%up.67 (tbytes 32)) (call
                                    %upgradeFromTransient.65
                                    (var-ref %f.66))))
       (let* (((%f2.68 (tfield (field-native))) (call
                                                  %degradeToTransient.63
                                                  (var-ref %up.67))))
         (seq (let* (((%tmp.69 (tfield (field-native))) (+ (tfield
                                                             (field-native))
                                                           (var-ref %f.66)
                                                           (var-ref
                                                             %f2.68))))
                (public-ledger %scratch.26 write (0) write (ttuple)
                  (instructions
                    (push
                      (storage #f)
                      (value (state-value cell (align 0 1))))
                    (push
                      (storage #t)
                      (value (state-value cell (var-ref %tmp.69))))
                    (ins (cached #f) (n 1)))
                  (var-ref %tmp.69)))
              (return (var-ref %f2.68)))))))
 (circuit %curve_ops.31 (exported #t) (pure #f) (proof #t)
   ((%s.44 (tfield (field-native)))
     (%m.46 (tfield (field-native))))
   (tfield (field-native))
   (let* (((%g.48 (tpoint (curve-jubjub))) (call
                                             %ecMulGenerator.43
                                             (cast-to-field
                                               (field-scalar
                                                 (curve-jubjub))
                                               (tfield (field-native))
                                               (var-ref %s.44)))))
     (let* (((%h.49 (tpoint (curve-jubjub))) (call
                                               %hashToCurve.45
                                               (tuple
                                                 (single (var-ref %s.44))
                                                 (single
                                                   (var-ref %m.46))))))
       (let* (((%sum.51 (tpoint (curve-jubjub))) (call
                                                   %ecAdd.47
                                                   (var-ref %g.48)
                                                   (var-ref %h.49))))
         (let* (((%prod.53 (tpoint (curve-jubjub))) (call
                                                      %ecMul.50
                                                      (var-ref %sum.51)
                                                      (cast-to-field
                                                        (field-scalar
                                                          (curve-jubjub))
                                                        (tfield
                                                          (field-native))
                                                        (var-ref %m.46)))))
           (let* (((%packed.55 (tfield (field-native))) (+ (tfield
                                                             (field-native))
                                                           (call
                                                             %jubjubPointX.52
                                                             (var-ref
                                                               %prod.53))
                                                           (call
                                                             %jubjubPointY.54
                                                             (var-ref
                                                               %prod.53)))))
             (seq (public-ledger %scratch.26 write (0) write (ttuple)
                    (instructions
                      (push
                        (storage #f)
                        (value (state-value cell (align 0 1))))
                      (push
                        (storage #t)
                        (value (state-value cell (var-ref %packed.55))))
                      (ins (cached #f) (n 1)))
                    (var-ref %packed.55))
                  (return (var-ref %packed.55)))))))))
 (circuit %curve_neg.34 (exported #t) (pure #f) (proof #t)
   ((%s.57 (tfield (field-native)))) (tfield (field-native))
   (let* (((%p.58 (tpoint (curve-jubjub))) (call
                                             %ecNeg.56
                                             (call
                                               %ecMulGenerator.43
                                               (cast-to-field
                                                 (field-scalar
                                                   (curve-jubjub))
                                                 (tfield (field-native))
                                                 (var-ref %s.57))))))
     (let* (((%packed.59 (tfield (field-native))) (+ (tfield
                                                       (field-native))
                                                     (call
                                                       %jubjubPointX.52
                                                       (var-ref %p.58))
                                                     (call
                                                       %jubjubPointY.54
                                                       (var-ref %p.58)))))
       (seq (public-ledger %scratch.26 write (0) write (ttuple)
              (instructions
                (push (storage #f) (value (state-value cell (align 0 1))))
                (push
                  (storage #t)
                  (value (state-value cell (var-ref %packed.59))))
                (ins (cached #f) (n 1)))
              (var-ref %packed.59))
            (return (var-ref %packed.59))))))
 (circuit %ledger_ops.27 (exported #t) (pure #f) (proof #t)
   ((%k.37 (tfield (field-native)))
     (%v.38 (tunsigned 18446744073709551615))
     (%entry.35 (tbytes 32)))
   (tboolean)
   (seq (let* (((%tmp.36 (tunsigned 65535)) (safe-cast
                                              (tunsigned 65535)
                                              (tunsigned 1)
                                              '1)))
          (public-ledger %hits.30 update (4) increment (ttuple)
            (instructions
              (idx (cached #f) (pushPath #t) (path ((align 4 1))))
              (addi (immediate (value->int (var-ref %tmp.36))))
              (ins (cached #t) (n 1)))
            (var-ref %tmp.36)))
        (public-ledger %scores.25 update (3) insert (ttuple)
          (instructions (idx (cached #f) (pushPath #t) (path ((align 3 1))))
            (push
              (storage #f)
              (value (state-value cell (var-ref %k.37))))
            (push
              (storage #t)
              (value
                (state-value
                  ADT
                  (var-ref %v.38)
                  (tunsigned 18446744073709551615))))
            (ins (cached #f) (n 1)) (ins (cached #t) (n 1)))
          (var-ref %k.37) (var-ref %v.38))
        (public-ledger %seen.23 update (2) insert (ttuple)
          (instructions (idx (cached #f) (pushPath #t) (path ((align 2 1))))
            (push
              (storage #f)
              (value (state-value cell (var-ref %entry.35))))
            (push (storage #t) (value (state-value null)))
            (ins (cached #f) (n 1)) (ins (cached #t) (n 1)))
          (var-ref %entry.35))
        (return
          (public-ledger %seen.23 read (2) member (tboolean)
            (instructions (dup (n 0))
              (idx (cached #f) (pushPath #f) (path ((align 2 1))))
              (push
                (storage #f)
                (value (state-value cell (var-ref %entry.35))))
              (member) (popeq (cached #t) (result (void))))
            (var-ref %entry.35)))))
 (circuit %casts.33 (exported #t) (pure #f) (proof #t)
   ((%n.39 (tunsigned 4294967295))
     (%f.40 (tfield (field-native))))
   (tbytes 32)
   (let* (((%wide.1 (tunsigned 36893488147419103230)) (+ (tunsigned
                                                           36893488147419103230)
                                                         (safe-cast
                                                           (tunsigned
                                                             36893488147419103230)
                                                           (tunsigned
                                                             18446744073709551615)
                                                           (safe-cast
                                                             (tunsigned
                                                               18446744073709551615)
                                                             (tunsigned
                                                               4294967295)
                                                             (var-ref
                                                               %n.39)))
                                                         (safe-cast
                                                           (tunsigned
                                                             36893488147419103230)
                                                           (tunsigned
                                                             18446744073709551615)
                                                           (safe-cast
                                                             (tunsigned
                                                               18446744073709551615)
                                                             (tunsigned 1)
                                                             '1)))))
     (let* (((%as_field.41 (tfield (field-native))) (safe-cast
                                                      (tfield
                                                        (field-native))
                                                      (tunsigned
                                                        36893488147419103230)
                                                      (var-ref %wide.1))))
       (let* (((%b.42 (tbytes 32)) (field->bytes
                                     32
                                     (field-native)
                                     (+ (tfield (field-native))
                                        (var-ref %f.40)
                                        (var-ref %as_field.41)))))
         (seq (public-ledger %tag_cell.24 write (1) write (ttuple)
                (instructions
                  (push
                    (storage #f)
                    (value (state-value cell (align 1 1))))
                  (push
                    (storage #t)
                    (value (state-value cell (var-ref %b.42))))
                  (ins (cached #f) (n 1)))
                (var-ref %b.42))
              (return (var-ref %b.42))))))))
