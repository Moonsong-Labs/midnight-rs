(analyzed-ir (compiler-version "0.35.103-dev")
 (language-version "0.27.0") (runtime-version "0.20.101")
 (exports (casts . %casts.32) (curve_neg . %curve_neg.33)
   (curve_ops . %curve_ops.30) (field_arith . %field_arith.31)
   (field_reduce . %field_reduce.28) (hits . %hits.29)
   (ledger_ops . %ledger_ops.26)
   (persistent_hashes . %persistent_hashes.27)
   (scores . %scores.24) (scratch . %scratch.25)
   (seen . %seen.22) (tag_cell . %tag_cell.23)
   (transient_conversions . %transient_conversions.20)
   (transient_hashes . %transient_hashes.21))
 (contract-types)
 (kernel-declaration (%kernel.83 () (exported #f) (Kernel)))
 (public-ledger-declaration
   (public-ledger-array
     (%scratch.25
       (0)
       (exported #t)
       (__compact_Cell (tfield (field-native))))
     (%tag_cell.23
       (1)
       (exported #t)
       (__compact_Cell (tbytes 32)))
     (%seen.22 (2) (exported #t) (Set (tbytes 32)))
     (%scores.24
       (3)
       (exported #t)
       (Map (tfield (field-native))
            (tunsigned 18446744073709551615)))
     (%hits.29 (4) (exported #t) (Counter)))
   (constructor () (tuple)))
 (native %transientHash.74
   (entry "__compactRuntime.transientHash" circuit)
   (type-arguments (tvector 2 (tfield (field-native))))
   ((%value.84 (tvector 2 (tfield (field-native)))))
   (tfield (field-native)))
 (native %transientCommit.77
   (entry "__compactRuntime.transientCommit" circuit)
   (type-arguments (tfield (field-native)))
   ((%value.85 (tfield (field-native)))
     (%rand.86 (tfield (field-native))))
   (tfield (field-native)))
 (native %persistentHash.59
   (entry "__compactRuntime.persistentHash" circuit)
   (type-arguments (tvector 2 (tbytes 32)))
   ((%value.87 (tvector 2 (tbytes 32)))) (tbytes 32))
 (native %persistentCommit.61
   (entry "__compactRuntime.persistentCommit" circuit)
   (type-arguments (tbytes 32))
   ((%value.88 (tbytes 32)) (%rand.89 (tbytes 32)))
   (tbytes 32))
 (native %degradeToTransient.64
   (entry "__compactRuntime.degradeToTransient" circuit)
   (type-arguments) ((%x.90 (tbytes 32)))
   (tfield (field-native)))
 (native %upgradeFromTransient.66
   (entry "__compactRuntime.upgradeFromTransient" circuit)
   (type-arguments) ((%x.91 (tfield (field-native))))
   (tbytes 32))
 (native %jubjubPointX.51
   (entry "__compactRuntime.jubjubPointX" circuit)
   (type-arguments) ((%pt.92 (tpoint (curve-jubjub))))
   (tfield (field-native)))
 (native %jubjubPointY.53
   (entry "__compactRuntime.jubjubPointY" circuit)
   (type-arguments) ((%pt.93 (tpoint (curve-jubjub))))
   (tfield (field-native)))
 (native %ecAdd.46 (entry "__compactRuntime.ecAdd" circuit)
   (type-arguments)
   ((%a.94 (tpoint (curve-jubjub)))
     (%b.95 (tpoint (curve-jubjub))))
   (tpoint (curve-jubjub)))
 (native %ecNeg.55 (entry "__compactRuntime.ecNeg" circuit)
   (type-arguments) ((%a.96 (tpoint (curve-jubjub))))
   (tpoint (curve-jubjub)))
 (native %ecMul.49 (entry "__compactRuntime.ecMul" circuit)
   (type-arguments)
   ((%a.97 (tpoint (curve-jubjub)))
     (%b.98 (tfield (field-scalar (curve-jubjub)))))
   (tpoint (curve-jubjub)))
 (native %ecMulGenerator.42
   (entry "__compactRuntime.ecMulGenerator" circuit)
   (type-arguments)
   ((%b.99 (tfield (field-scalar (curve-jubjub)))))
   (tpoint (curve-jubjub)))
 (native %hashToCurve.44
   (entry "__compactRuntime.hashToCurve" circuit)
   (type-arguments (tvector 2 (tfield (field-native))))
   ((%value.100 (tvector 2 (tfield (field-native)))))
   (tpoint (curve-jubjub)))
 (circuit %field_arith.31 (exported #t) (pure #f) (proof #t)
   ((%a.80 (tfield (field-native)))
     (%b.81 (tfield (field-native))))
   (tfield (field-native))
   (let* (((%p.18 (tfield (field-native))) (* (tfield
                                                (field-native))
                                              (var-ref %a.80)
                                              (var-ref %b.81))))
     (let* (((%s.19 (tfield (field-native))) (+ (tfield
                                                  (field-native))
                                                (var-ref %p.18)
                                                (var-ref %a.80))))
       (let* (((%d.82 (tfield (field-native))) (- (tfield
                                                    (field-native))
                                                  (var-ref %s.19)
                                                  (var-ref %b.81))))
         (seq (public-ledger %scratch.25 write (0) write (ttuple)
                (instructions
                  (push
                    (storage #f)
                    (value (state-value cell (align 0 1))))
                  (push
                    (storage #t)
                    (value (state-value cell (var-ref %d.82))))
                  (ins (cached #f) (n 1)))
                (var-ref %d.82))
              (return (var-ref %d.82)))))))
 (circuit %field_reduce.28 (exported #t) (pure #f) (proof #t)
   ((%c.71 (tfield (field-native)))
     (%q.72 (tfield (field-native))))
   (tfield (field-native))
   (let* (((%r.73 (tfield (field-native))) (- (tfield
                                                (field-native))
                                              (var-ref %c.71)
                                              (* (tfield (field-native))
                                                 (var-ref %q.72)
                                                 '6554484396890773809930967563523245729705921265872317281365359162392183254199))))
     (seq (public-ledger %scratch.25 write (0) write (ttuple)
            (instructions
              (push (storage #f) (value (state-value cell (align 0 1))))
              (push
                (storage #t)
                (value (state-value cell (var-ref %r.73))))
              (ins (cached #f) (n 1)))
            (var-ref %r.73))
          (return (var-ref %r.73)))))
 (circuit %transient_hashes.21 (exported #t) (pure #f) (proof #t)
   ((%x.75 (tfield (field-native)))
     (%r.76 (tfield (field-native))))
   (tfield (field-native))
   (let* (((%h.78 (tfield (field-native))) (call
                                             %transientHash.74
                                             (tuple
                                               (single (var-ref %x.75))
                                               (single (var-ref %r.76))))))
     (let* (((%c.79 (tfield (field-native))) (call
                                               %transientCommit.77
                                               (var-ref %h.78)
                                               (var-ref %r.76))))
       (seq (public-ledger %scratch.25 write (0) write (ttuple)
              (instructions
                (push (storage #f) (value (state-value cell (align 0 1))))
                (push
                  (storage #t)
                  (value (state-value cell (var-ref %c.79))))
                (ins (cached #f) (n 1)))
              (var-ref %c.79))
            (return (var-ref %c.79))))))
 (circuit %persistent_hashes.27 (exported #t) (pure #f) (proof #t)
   ((%x.60 (tbytes 32))) (tbytes 32)
   (let* (((%h.62 (tbytes 32)) (call
                                 %persistentHash.59
                                 (tuple
                                   (single
                                     '#vu8(111 112 115 58 112 104 58 0 0 0
                                           0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0
                                           0 0 0 0 0 0))
                                   (single (var-ref %x.60))))))
     (let* (((%c.63 (tbytes 32)) (call
                                   %persistentCommit.61
                                   (var-ref %h.62)
                                   (var-ref %x.60))))
       (seq (public-ledger %tag_cell.23 write (1) write (ttuple)
              (instructions
                (push (storage #f) (value (state-value cell (align 1 1))))
                (push
                  (storage #t)
                  (value (state-value cell (var-ref %c.63))))
                (ins (cached #f) (n 1)))
              (var-ref %c.63))
            (return (var-ref %c.63))))))
 (circuit %transient_conversions.20 (exported #t) (pure #f)
   (proof #t) ((%x.65 (tbytes 32))) (tfield (field-native))
   (let* (((%f.67 (tfield (field-native))) (call
                                             %degradeToTransient.64
                                             (var-ref %x.65))))
     (let* (((%up.68 (tbytes 32)) (call
                                    %upgradeFromTransient.66
                                    (var-ref %f.67))))
       (let* (((%f2.69 (tfield (field-native))) (call
                                                  %degradeToTransient.64
                                                  (var-ref %up.68))))
         (seq (let* (((%tmp.70 (tfield (field-native))) (+ (tfield
                                                             (field-native))
                                                           (var-ref %f.67)
                                                           (var-ref
                                                             %f2.69))))
                (public-ledger %scratch.25 write (0) write (ttuple)
                  (instructions
                    (push
                      (storage #f)
                      (value (state-value cell (align 0 1))))
                    (push
                      (storage #t)
                      (value (state-value cell (var-ref %tmp.70))))
                    (ins (cached #f) (n 1)))
                  (var-ref %tmp.70)))
              (return (var-ref %f2.69)))))))
 (circuit %curve_ops.30 (exported #t) (pure #f) (proof #t)
   ((%s.43 (tfield (field-native)))
     (%m.45 (tfield (field-native))))
   (tfield (field-native))
   (let* (((%g.47 (tpoint (curve-jubjub))) (call
                                             %ecMulGenerator.42
                                             (cast-to-field
                                               (field-scalar
                                                 (curve-jubjub))
                                               (tfield (field-native))
                                               (var-ref %s.43)))))
     (let* (((%h.48 (tpoint (curve-jubjub))) (call
                                               %hashToCurve.44
                                               (tuple
                                                 (single (var-ref %s.43))
                                                 (single
                                                   (var-ref %m.45))))))
       (let* (((%sum.50 (tpoint (curve-jubjub))) (call
                                                   %ecAdd.46
                                                   (var-ref %g.47)
                                                   (var-ref %h.48))))
         (let* (((%prod.52 (tpoint (curve-jubjub))) (call
                                                      %ecMul.49
                                                      (var-ref %sum.50)
                                                      (cast-to-field
                                                        (field-scalar
                                                          (curve-jubjub))
                                                        (tfield
                                                          (field-native))
                                                        (var-ref %m.45)))))
           (let* (((%packed.54 (tfield (field-native))) (+ (tfield
                                                             (field-native))
                                                           (call
                                                             %jubjubPointX.51
                                                             (var-ref
                                                               %prod.52))
                                                           (call
                                                             %jubjubPointY.53
                                                             (var-ref
                                                               %prod.52)))))
             (seq (public-ledger %scratch.25 write (0) write (ttuple)
                    (instructions
                      (push
                        (storage #f)
                        (value (state-value cell (align 0 1))))
                      (push
                        (storage #t)
                        (value (state-value cell (var-ref %packed.54))))
                      (ins (cached #f) (n 1)))
                    (var-ref %packed.54))
                  (return (var-ref %packed.54)))))))))
 (circuit %curve_neg.33 (exported #t) (pure #f) (proof #t)
   ((%s.56 (tfield (field-native)))) (tfield (field-native))
   (let* (((%p.57 (tpoint (curve-jubjub))) (call
                                             %ecNeg.55
                                             (call
                                               %ecMulGenerator.42
                                               (cast-to-field
                                                 (field-scalar
                                                   (curve-jubjub))
                                                 (tfield (field-native))
                                                 (var-ref %s.56))))))
     (let* (((%packed.58 (tfield (field-native))) (+ (tfield
                                                       (field-native))
                                                     (call
                                                       %jubjubPointX.51
                                                       (var-ref %p.57))
                                                     (call
                                                       %jubjubPointY.53
                                                       (var-ref %p.57)))))
       (seq (public-ledger %scratch.25 write (0) write (ttuple)
              (instructions
                (push (storage #f) (value (state-value cell (align 0 1))))
                (push
                  (storage #t)
                  (value (state-value cell (var-ref %packed.58))))
                (ins (cached #f) (n 1)))
              (var-ref %packed.58))
            (return (var-ref %packed.58))))))
 (circuit %ledger_ops.26 (exported #t) (pure #f) (proof #t)
   ((%k.36 (tfield (field-native)))
     (%v.37 (tunsigned 18446744073709551615))
     (%entry.34 (tbytes 32)))
   (tboolean)
   (seq (let* (((%tmp.35 (tunsigned 65535)) (safe-cast
                                              (tunsigned 65535)
                                              (tunsigned 1)
                                              '1)))
          (public-ledger %hits.29 update (4) increment (ttuple)
            (instructions
              (idx (cached #f) (pushPath #t) (path ((align 4 1))))
              (addi (immediate (value->int (var-ref %tmp.35))))
              (ins (cached #t) (n 1)))
            (var-ref %tmp.35)))
        (public-ledger %scores.24 update (3) insert (ttuple)
          (instructions (idx (cached #f) (pushPath #t) (path ((align 3 1))))
            (push
              (storage #f)
              (value (state-value cell (var-ref %k.36))))
            (push
              (storage #t)
              (value
                (state-value
                  ADT
                  (var-ref %v.37)
                  (tunsigned 18446744073709551615))))
            (ins (cached #f) (n 1)) (ins (cached #t) (n 1)))
          (var-ref %k.36) (var-ref %v.37))
        (public-ledger %seen.22 update (2) insert (ttuple)
          (instructions (idx (cached #f) (pushPath #t) (path ((align 2 1))))
            (push
              (storage #f)
              (value (state-value cell (var-ref %entry.34))))
            (push (storage #t) (value (state-value null)))
            (ins (cached #f) (n 1)) (ins (cached #t) (n 1)))
          (var-ref %entry.34))
        (return
          (public-ledger %seen.22 read (2) member (tboolean)
            (instructions (dup (n 0))
              (idx (cached #f) (pushPath #f) (path ((align 2 1))))
              (push
                (storage #f)
                (value (state-value cell (var-ref %entry.34))))
              (member) (popeq (cached #t) (result (void))))
            (var-ref %entry.34)))))
 (circuit %casts.32 (exported #t) (pure #f) (proof #t)
   ((%n.38 (tunsigned 4294967295))
     (%f.39 (tfield (field-native))))
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
                                                               %n.38)))
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
     (let* (((%as_field.40 (tfield (field-native))) (safe-cast
                                                      (tfield
                                                        (field-native))
                                                      (tunsigned
                                                        36893488147419103230)
                                                      (var-ref %wide.1))))
       (let* (((%b.41 (tbytes 32)) (field->bytes
                                     32
                                     (field-native)
                                     (+ (tfield (field-native))
                                        (var-ref %f.39)
                                        (var-ref %as_field.40)))))
         (seq (public-ledger %tag_cell.23 write (1) write (ttuple)
                (instructions
                  (push
                    (storage #f)
                    (value (state-value cell (align 1 1))))
                  (push
                    (storage #t)
                    (value (state-value cell (var-ref %b.41))))
                  (ins (cached #f) (n 1)))
                (var-ref %b.41))
              (return (var-ref %b.41))))))))
