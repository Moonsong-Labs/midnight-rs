(analyzed-ir (compiler-version "0.35.103-dev")
 (language-version "0.27.0") (runtime-version "0.20.101")
 (exports (casts . %casts.47)
   (counter_as_field . %counter_as_field.48)
   (curve_neg . %curve_neg.45) (curve_ops . %curve_ops.46)
   (field_arith . %field_arith.43)
   (field_reduce . %field_reduce.44) (hits . %hits.41)
   (ledger_ops . %ledger_ops.42) (narrow . %narrow.39)
   (persistent_hashes . %persistent_hashes.40)
   (scores . %scores.37) (scratch . %scratch.38)
   (seen . %seen.35) (tag_cell . %tag_cell.36)
   (transient_conversions . %transient_conversions.33)
   (transient_hashes . %transient_hashes.34) (tri . %tri.31)
   (uint_hash . %uint_hash.32))
 (contract-types)
 (kernel-declaration (%kernel.111 () (exported #f) (Kernel)))
 (public-ledger-declaration
   (public-ledger-array
     (%scratch.38
       (0)
       (exported #t)
       (__compact_Cell (tfield (field-native))))
     (%tag_cell.36
       (1)
       (exported #t)
       (__compact_Cell (tbytes 32)))
     (%seen.35 (2) (exported #t) (Set (tbytes 32)))
     (%scores.37
       (3)
       (exported #t)
       (Map (tfield (field-native))
            (tunsigned 18446744073709551615)))
     (%hits.41 (4) (exported #t) (Counter)))
   (constructor () (tuple)))
 (export-typedef Tri () (tenum Tri low mid high))
 (native %transientHash.102
   (entry "__compactRuntime.transientHash" circuit)
   (type-arguments (tvector 2 (tfield (field-native))))
   ((%value.112 (tvector 2 (tfield (field-native)))))
   (tfield (field-native)))
 (native %transientCommit.105
   (entry "__compactRuntime.transientCommit" circuit)
   (type-arguments (tfield (field-native)))
   ((%value.113 (tfield (field-native)))
     (%rand.114 (tfield (field-native))))
   (tfield (field-native)))
 (native %persistentHash.91
   (entry "__compactRuntime.persistentHash" circuit)
   (type-arguments (tvector 2 (tbytes 32)))
   ((%value.115 (tvector 2 (tbytes 32)))) (tbytes 32))
 (native %persistentHash.96
   (entry "__compactRuntime.persistentHash" circuit)
   (type-arguments (tunsigned 18446744073709551615))
   ((%value.116 (tunsigned 18446744073709551615))) (tbytes 32))
 (native %persistentCommit.93
   (entry "__compactRuntime.persistentCommit" circuit)
   (type-arguments (tbytes 32))
   ((%value.117 (tbytes 32)) (%rand.118 (tbytes 32)))
   (tbytes 32))
 (native %degradeToTransient.74
   (entry "__compactRuntime.degradeToTransient" circuit)
   (type-arguments) ((%x.119 (tbytes 32)))
   (tfield (field-native)))
 (native %upgradeFromTransient.76
   (entry "__compactRuntime.upgradeFromTransient" circuit)
   (type-arguments) ((%x.120 (tfield (field-native))))
   (tbytes 32))
 (native %jubjubPointX.66
   (entry "__compactRuntime.jubjubPointX" circuit)
   (type-arguments) ((%pt.121 (tpoint (curve-jubjub))))
   (tfield (field-native)))
 (native %jubjubPointY.68
   (entry "__compactRuntime.jubjubPointY" circuit)
   (type-arguments) ((%pt.122 (tpoint (curve-jubjub))))
   (tfield (field-native)))
 (native %ecAdd.84 (entry "__compactRuntime.ecAdd" circuit)
   (type-arguments)
   ((%a.123 (tpoint (curve-jubjub)))
     (%b.124 (tpoint (curve-jubjub))))
   (tpoint (curve-jubjub)))
 (native %ecNeg.63 (entry "__compactRuntime.ecNeg" circuit)
   (type-arguments) ((%a.125 (tpoint (curve-jubjub))))
   (tpoint (curve-jubjub)))
 (native %ecMul.87 (entry "__compactRuntime.ecMul" circuit)
   (type-arguments)
   ((%a.126 (tpoint (curve-jubjub)))
     (%b.127 (tfield (field-scalar (curve-jubjub)))))
   (tpoint (curve-jubjub)))
 (native %ecMulGenerator.64
   (entry "__compactRuntime.ecMulGenerator" circuit)
   (type-arguments)
   ((%b.128 (tfield (field-scalar (curve-jubjub)))))
   (tpoint (curve-jubjub)))
 (native %hashToCurve.82
   (entry "__compactRuntime.hashToCurve" circuit)
   (type-arguments (tvector 2 (tfield (field-native))))
   ((%value.129 (tvector 2 (tfield (field-native)))))
   (tpoint (curve-jubjub)))
 (circuit %field_arith.43 (exported #t) (pure #f) (proof #t)
   ((%a.108 (tfield (field-native)))
     (%b.109 (tfield (field-native))))
   (tfield (field-native))
   (let* (((%p.26 (tfield (field-native))) (* (tfield
                                                (field-native))
                                              (var-ref %a.108)
                                              (var-ref %b.109))))
     (let* (((%s.27 (tfield (field-native))) (+ (tfield
                                                  (field-native))
                                                (var-ref %p.26)
                                                (var-ref %a.108))))
       (let* (((%d.110 (tfield (field-native))) (- (tfield
                                                     (field-native))
                                                   (var-ref %s.27)
                                                   (var-ref %b.109))))
         (seq (public-ledger %scratch.38 write (0) write (ttuple)
                (instructions
                  (push
                    (storage #f)
                    (value (state-value cell (align 0 1))))
                  (push
                    (storage #t)
                    (value (state-value cell (var-ref %d.110))))
                  (ins (cached #f) (n 1)))
                (var-ref %d.110))
              (return (var-ref %d.110)))))))
 (circuit %field_reduce.44 (exported #t) (pure #f) (proof #t)
   ((%c.99 (tfield (field-native)))
     (%q.100 (tfield (field-native))))
   (tfield (field-native))
   (let* (((%r.101 (tfield (field-native))) (- (tfield
                                                 (field-native))
                                               (var-ref %c.99)
                                               (* (tfield (field-native))
                                                  (var-ref %q.100)
                                                  '6554484396890773809930967563523245729705921265872317281365359162392183254199))))
     (seq (public-ledger %scratch.38 write (0) write (ttuple)
            (instructions
              (push (storage #f) (value (state-value cell (align 0 1))))
              (push
                (storage #t)
                (value (state-value cell (var-ref %r.101))))
              (ins (cached #f) (n 1)))
            (var-ref %r.101))
          (return (var-ref %r.101)))))
 (circuit %transient_hashes.34 (exported #t) (pure #f) (proof #t)
   ((%x.103 (tfield (field-native)))
     (%r.104 (tfield (field-native))))
   (tfield (field-native))
   (let* (((%h.106 (tfield (field-native))) (call
                                              %transientHash.102
                                              (tuple
                                                (single (var-ref %x.103))
                                                (single
                                                  (var-ref %r.104))))))
     (let* (((%c.107 (tfield (field-native))) (call
                                                %transientCommit.105
                                                (var-ref %h.106)
                                                (var-ref %r.104))))
       (seq (public-ledger %scratch.38 write (0) write (ttuple)
              (instructions
                (push (storage #f) (value (state-value cell (align 0 1))))
                (push
                  (storage #t)
                  (value (state-value cell (var-ref %c.107))))
                (ins (cached #f) (n 1)))
              (var-ref %c.107))
            (return (var-ref %c.107))))))
 (circuit %persistent_hashes.40 (exported #t) (pure #f) (proof #t)
   ((%x.92 (tbytes 32))) (tbytes 32)
   (let* (((%h.94 (tbytes 32)) (call
                                 %persistentHash.91
                                 (tuple
                                   (single
                                     '#vu8(111 112 115 58 112 104 58 0 0 0
                                           0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0
                                           0 0 0 0 0 0))
                                   (single (var-ref %x.92))))))
     (let* (((%c.95 (tbytes 32)) (call
                                   %persistentCommit.93
                                   (var-ref %h.94)
                                   (var-ref %x.92))))
       (seq (public-ledger %tag_cell.36 write (1) write (ttuple)
              (instructions
                (push (storage #f) (value (state-value cell (align 1 1))))
                (push
                  (storage #t)
                  (value (state-value cell (var-ref %c.95))))
                (ins (cached #f) (n 1)))
              (var-ref %c.95))
            (return (var-ref %c.95))))))
 (circuit %uint_hash.32 (exported #t) (pure #f) (proof #t)
   ((%n.97 (tunsigned 18446744073709551615))) (tbytes 32)
   (let* (((%h.98 (tbytes 32)) (call
                                 %persistentHash.96
                                 (var-ref %n.97))))
     (seq (public-ledger %tag_cell.36 write (1) write (ttuple)
            (instructions
              (push (storage #f) (value (state-value cell (align 1 1))))
              (push
                (storage #t)
                (value (state-value cell (var-ref %h.98))))
              (ins (cached #f) (n 1)))
            (var-ref %h.98))
          (return (var-ref %h.98)))))
 (circuit %transient_conversions.33 (exported #t) (pure #f)
   (proof #t) ((%x.75 (tbytes 32))) (tfield (field-native))
   (let* (((%f.77 (tfield (field-native))) (call
                                             %degradeToTransient.74
                                             (var-ref %x.75))))
     (let* (((%up.78 (tbytes 32)) (call
                                    %upgradeFromTransient.76
                                    (var-ref %f.77))))
       (let* (((%f2.79 (tfield (field-native))) (call
                                                  %degradeToTransient.74
                                                  (var-ref %up.78))))
         (seq (let* (((%tmp.80 (tfield (field-native))) (+ (tfield
                                                             (field-native))
                                                           (var-ref %f.77)
                                                           (var-ref
                                                             %f2.79))))
                (public-ledger %scratch.38 write (0) write (ttuple)
                  (instructions
                    (push
                      (storage #f)
                      (value (state-value cell (align 0 1))))
                    (push
                      (storage #t)
                      (value (state-value cell (var-ref %tmp.80))))
                    (ins (cached #f) (n 1)))
                  (var-ref %tmp.80)))
              (return (var-ref %f2.79)))))))
 (circuit %curve_ops.46 (exported #t) (pure #f) (proof #t)
   ((%s.81 (tfield (field-native)))
     (%m.83 (tfield (field-native))))
   (tfield (field-native))
   (let* (((%g.85 (tpoint (curve-jubjub))) (call
                                             %ecMulGenerator.64
                                             (cast-to-field
                                               (field-scalar
                                                 (curve-jubjub))
                                               (tfield (field-native))
                                               (var-ref %s.81)))))
     (let* (((%h.86 (tpoint (curve-jubjub))) (call
                                               %hashToCurve.82
                                               (tuple
                                                 (single (var-ref %s.81))
                                                 (single
                                                   (var-ref %m.83))))))
       (let* (((%sum.88 (tpoint (curve-jubjub))) (call
                                                   %ecAdd.84
                                                   (var-ref %g.85)
                                                   (var-ref %h.86))))
         (let* (((%prod.89 (tpoint (curve-jubjub))) (call
                                                      %ecMul.87
                                                      (var-ref %sum.88)
                                                      (cast-to-field
                                                        (field-scalar
                                                          (curve-jubjub))
                                                        (tfield
                                                          (field-native))
                                                        (var-ref %m.83)))))
           (let* (((%packed.90 (tfield (field-native))) (+ (tfield
                                                             (field-native))
                                                           (call
                                                             %jubjubPointX.66
                                                             (var-ref
                                                               %prod.89))
                                                           (call
                                                             %jubjubPointY.68
                                                             (var-ref
                                                               %prod.89)))))
             (seq (public-ledger %scratch.38 write (0) write (ttuple)
                    (instructions
                      (push
                        (storage #f)
                        (value (state-value cell (align 0 1))))
                      (push
                        (storage #t)
                        (value (state-value cell (var-ref %packed.90))))
                      (ins (cached #f) (n 1)))
                    (var-ref %packed.90))
                  (return (var-ref %packed.90)))))))))
 (circuit %curve_neg.45 (exported #t) (pure #f) (proof #t)
   ((%s.65 (tfield (field-native)))) (tfield (field-native))
   (let* (((%p.67 (tpoint (curve-jubjub))) (call
                                             %ecNeg.63
                                             (call
                                               %ecMulGenerator.64
                                               (cast-to-field
                                                 (field-scalar
                                                   (curve-jubjub))
                                                 (tfield (field-native))
                                                 (var-ref %s.65))))))
     (let* (((%packed.69 (tfield (field-native))) (+ (tfield
                                                       (field-native))
                                                     (call
                                                       %jubjubPointX.66
                                                       (var-ref %p.67))
                                                     (call
                                                       %jubjubPointY.68
                                                       (var-ref %p.67)))))
       (seq (public-ledger %scratch.38 write (0) write (ttuple)
              (instructions
                (push (storage #f) (value (state-value cell (align 0 1))))
                (push
                  (storage #t)
                  (value (state-value cell (var-ref %packed.69))))
                (ins (cached #f) (n 1)))
              (var-ref %packed.69))
            (return (var-ref %packed.69))))))
 (circuit %ledger_ops.42 (exported #t) (pure #f) (proof #t)
   ((%k.72 (tfield (field-native)))
     (%v.73 (tunsigned 18446744073709551615))
     (%entry.70 (tbytes 32)))
   (tboolean)
   (seq (let* (((%tmp.71 (tunsigned 65535)) (safe-cast
                                              (tunsigned 65535)
                                              (tunsigned 1)
                                              '1)))
          (public-ledger %hits.41 update (4) increment (ttuple)
            (instructions
              (idx (cached #f) (pushPath #t) (path ((align 4 1))))
              (addi (immediate (value->int (var-ref %tmp.71))))
              (ins (cached #t) (n 1)))
            (var-ref %tmp.71)))
        (public-ledger %scores.37 update (3) insert (ttuple)
          (instructions (idx (cached #f) (pushPath #t) (path ((align 3 1))))
            (push
              (storage #f)
              (value (state-value cell (var-ref %k.72))))
            (push
              (storage #t)
              (value
                (state-value
                  ADT
                  (var-ref %v.73)
                  (tunsigned 18446744073709551615))))
            (ins (cached #f) (n 1)) (ins (cached #t) (n 1)))
          (var-ref %k.72) (var-ref %v.73))
        (public-ledger %seen.35 update (2) insert (ttuple)
          (instructions (idx (cached #f) (pushPath #t) (path ((align 2 1))))
            (push
              (storage #f)
              (value (state-value cell (var-ref %entry.70))))
            (push (storage #t) (value (state-value null)))
            (ins (cached #f) (n 1)) (ins (cached #t) (n 1)))
          (var-ref %entry.70))
        (return
          (public-ledger %seen.35 read (2) member (tboolean)
            (instructions (dup (n 0))
              (idx (cached #f) (pushPath #f) (path ((align 2 1))))
              (push
                (storage #f)
                (value (state-value cell (var-ref %entry.70))))
              (member) (popeq (cached #t) (result (void))))
            (var-ref %entry.70)))))
 (circuit %casts.47 (exported #t) (pure #f) (proof #t)
   ((%n.54 (tunsigned 4294967295))
     (%f.55 (tfield (field-native))))
   (tbytes 32)
   (let* (((%wide.12 (tunsigned 36893488147419103230)) (+ (tunsigned
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
                                                                %n.54)))
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
     (let* (((%as_field.56 (tfield (field-native))) (safe-cast
                                                      (tfield
                                                        (field-native))
                                                      (tunsigned
                                                        36893488147419103230)
                                                      (var-ref %wide.12))))
       (let* (((%b.57 (tbytes 32)) (field->bytes
                                     32
                                     (field-native)
                                     (+ (tfield (field-native))
                                        (var-ref %f.55)
                                        (var-ref %as_field.56)))))
         (seq (public-ledger %tag_cell.36 write (1) write (ttuple)
                (instructions
                  (push
                    (storage #f)
                    (value (state-value cell (align 1 1))))
                  (push
                    (storage #t)
                    (value (state-value cell (var-ref %b.57))))
                  (ins (cached #f) (n 1)))
                (var-ref %b.57))
              (return (var-ref %b.57)))))))
 (circuit %narrow.39 (exported #t) (pure #f) (proof #t)
   ((%f.58 (tfield (field-native)))
     (%n.59 (tunsigned 65535))
     (%b.60 (tbytes 2)))
   (tunsigned 65535)
   (safe-cast
     (tunsigned 65535)
     (tunsigned 765)
     (let* (((%r.61 (tunsigned 765)) (+ (tunsigned 765)
                                        (safe-cast
                                          (tunsigned 765)
                                          (tunsigned 510)
                                          (+ (tunsigned 510)
                                             (safe-cast
                                               (tunsigned 510)
                                               (tunsigned 255)
                                               (cast-from-field
                                                 255
                                                 (field-native)
                                                 (var-ref %f.58)))
                                             (safe-cast
                                               (tunsigned 510)
                                               (tunsigned 255)
                                               (downcast-unsigned
                                                 65535
                                                 255
                                                 (var-ref %n.59)))))
                                        (safe-cast
                                          (tunsigned 765)
                                          (tunsigned 255)
                                          (cast-from-bytes
                                            (tunsigned 255)
                                            2
                                            (var-ref %b.60))))))
       (seq (let* (((%tmp.62 (tfield (field-native))) (safe-cast
                                                        (tfield
                                                          (field-native))
                                                        (tunsigned 765)
                                                        (var-ref %r.61))))
              (public-ledger %scratch.38 write (0) write (ttuple)
                (instructions
                  (push
                    (storage #f)
                    (value (state-value cell (align 0 1))))
                  (push
                    (storage #t)
                    (value (state-value cell (var-ref %tmp.62))))
                  (ins (cached #f) (n 1)))
                (var-ref %tmp.62)))
            (return (var-ref %r.61))))))
 (circuit %tri.31 (exported #t) (pure #f) (proof #t)
   ((%f.49 (tfield (field-native)))
     (%t.50 (tenum Tri low mid high)))
   (tunsigned 255)
   (safe-cast
     (tunsigned 255)
     (tunsigned 3)
     (let* (((%r.51 (tunsigned 3)) (+ (tunsigned 3)
                                      (safe-cast
                                        (tunsigned 3)
                                        (tunsigned 2)
                                        (cast-from-enum
                                          (tunsigned 2)
                                          (tenum Tri low mid high)
                                          (cast-to-enum
                                            (tenum Tri low mid high)
                                            (tfield (field-native))
                                            (var-ref %f.49))))
                                      (safe-cast
                                        (tunsigned 3)
                                        (tunsigned 1)
                                        (cast-from-enum
                                          (tunsigned 1)
                                          (tenum Tri low mid high)
                                          (var-ref %t.50))))))
       (seq (let* (((%tmp.52 (tfield (field-native))) (safe-cast
                                                        (tfield
                                                          (field-native))
                                                        (tunsigned 3)
                                                        (var-ref %r.51))))
              (public-ledger %scratch.38 write (0) write (ttuple)
                (instructions
                  (push
                    (storage #f)
                    (value (state-value cell (align 0 1))))
                  (push
                    (storage #t)
                    (value (state-value cell (var-ref %tmp.52))))
                  (ins (cached #f) (n 1)))
                (var-ref %tmp.52)))
            (return (var-ref %r.51))))))
 (circuit %counter_as_field.48 (exported #t) (pure #f) (proof #t) ()
   (tfield (field-native))
   (let* (((%f.53 (tfield (field-native))) (safe-cast
                                             (tfield (field-native))
                                             (tunsigned
                                               18446744073709551615)
                                             (public-ledger %hits.41 read (4) read
                                               (tunsigned
                                                 18446744073709551615)
                                               (instructions
                                                 (dup (n 0))
                                                 (idx (cached #f)
                                                      (pushPath #f)
                                                      (path ((align 4 1))))
                                                 (popeq
                                                   (cached #t)
                                                   (result (void))))))))
     (seq (public-ledger %scratch.38 write (0) write (ttuple)
            (instructions
              (push (storage #f) (value (state-value cell (align 0 1))))
              (push
                (storage #t)
                (value (state-value cell (var-ref %f.53))))
              (ins (cached #f) (n 1)))
            (var-ref %f.53))
          (return (var-ref %f.53))))))
