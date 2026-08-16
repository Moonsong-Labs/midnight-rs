(analyzed-ir (compiler-version "0.33.122")
 (language-version "0.25.107") (runtime-version "0.18.107")
 (exports (casts . %casts.28) (curve_ops . %curve_ops.29)
   (field_arith . %field_arith.26)
   (field_reduce . %field_reduce.27) (hits . %hits.24)
   (ledger_ops . %ledger_ops.25)
   (persistent_hashes . %persistent_hashes.22)
   (scores . %scores.23) (scratch . %scratch.20)
   (seen . %seen.21) (tag_cell . %tag_cell.18)
   (transient_conversions . %transient_conversions.19)
   (transient_hashes . %transient_hashes.17))
 (contract-types)
 (kernel-declaration (%kernel.91 () (exported #f) (Kernel)))
 (public-ledger-declaration
   (public-ledger-array
     (%scratch.20
       (0)
       (exported #t)
       (__compact_Cell (tfield (field-native))))
     (%tag_cell.18
       (1)
       (exported #t)
       (__compact_Cell (tbytes 32)))
     (%seen.21 (2) (exported #t) (Set (tbytes 32)))
     (%scores.23
       (3)
       (exported #t)
       (Map (tfield (field-native))
            (tunsigned 18446744073709551615)))
     (%hits.24 (4) (exported #t) (Counter)))
   (constructor () (tuple)))
 (native
   %transientHash.61
   (entry "__compactRuntime.transientHash" circuit)
   ((%value.88 (tvector 2 (tfield (field-native)))))
   (tfield (field-native)))
 (native
   %transientCommit.63
   (entry "__compactRuntime.transientCommit" circuit)
   ((%value.89 (tfield (field-native)))
     (%rand.90 (tfield (field-native))))
   (tfield (field-native)))
 (native
   %persistentHash.66
   (entry "__compactRuntime.persistentHash" circuit)
   ((%value.85 (tvector 2 (tbytes 32))))
   (tbytes 32))
 (native
   %persistentCommit.68
   (entry "__compactRuntime.persistentCommit" circuit)
   ((%value.86 (tbytes 32)) (%rand.87 (tbytes 32)))
   (tbytes 32))
 (native
   %degradeToTransient.40
   (entry "__compactRuntime.degradeToTransient" circuit)
   ((%x.83 (tbytes 32)))
   (tfield (field-native)))
 (native
   %upgradeFromTransient.42
   (entry "__compactRuntime.upgradeFromTransient" circuit)
   ((%x.84 (tfield (field-native))))
   (tbytes 32))
 (native
   %jubjubPointX.56
   (entry "__compactRuntime.jubjubPointX" circuit)
   ((%pt.81 (tpoint (curve-jubjub))))
   (tfield (field-native)))
 (native
   %jubjubPointY.57
   (entry "__compactRuntime.jubjubPointY" circuit)
   ((%pt.82 (tpoint (curve-jubjub))))
   (tfield (field-native)))
 (native
   %ecAdd.52
   (entry "__compactRuntime.ecAdd" circuit)
   ((%a.77 (tpoint (curve-jubjub)))
     (%b.78 (tpoint (curve-jubjub))))
   (tpoint (curve-jubjub)))
 (native
   %ecMul.54
   (entry "__compactRuntime.ecMul" circuit)
   ((%a.79 (tpoint (curve-jubjub)))
     (%b.80 (tfield (field-scalar (curve-jubjub)))))
   (tpoint (curve-jubjub)))
 (native
   %ecMulGenerator.48
   (entry "__compactRuntime.ecMulGenerator" circuit)
   ((%b.75 (tfield (field-scalar (curve-jubjub)))))
   (tpoint (curve-jubjub)))
 (native
   %hashToCurve.50
   (entry "__compactRuntime.hashToCurve" circuit)
   ((%value.76 (tvector 2 (tfield (field-native)))))
   (tpoint (curve-jubjub)))
 (circuit %field_arith.26 (exported #t) (pure #f) (proof #t)
   ((%a.69 (tfield (field-native)))
     (%b.70 (tfield (field-native))))
   (tfield (field-native))
   (let* (((%p.12 (tfield (field-native))) (* (tfield
                                                (field-native))
                                              (var-ref %a.69)
                                              (var-ref %b.70))))
     (let* (((%s.13 (tfield (field-native))) (+ (tfield
                                                  (field-native))
                                                (var-ref %p.12)
                                                (var-ref %a.69))))
       (let* (((%d.71 (tfield (field-native))) (- (tfield
                                                    (field-native))
                                                  (var-ref %s.13)
                                                  (var-ref %b.70))))
         (seq (public-ledger %scratch.20 (0) write (ttuple)
                (instructions
                  (push
                    (storage #f)
                    (value (state-value cell (align 0 1))))
                  (push
                    (storage #t)
                    (value (state-value cell (var-ref %d.71))))
                  (ins (cached #f) (n 1)))
                (var-ref %d.71))
              (return (var-ref %d.71)))))))
 (circuit %field_reduce.27 (exported #t) (pure #f) (proof #t)
   ((%c.72 (tfield (field-native)))
     (%q.73 (tfield (field-native))))
   (tfield (field-native))
   (let* (((%r.74 (tfield (field-native))) (- (tfield
                                                (field-native))
                                              (var-ref %c.72)
                                              (* (tfield (field-native))
                                                 (var-ref %q.73)
                                                 '6554484396890773809930967563523245729705921265872317281365359162392183254199))))
     (seq (public-ledger %scratch.20 (0) write (ttuple)
            (instructions
              (push (storage #f) (value (state-value cell (align 0 1))))
              (push
                (storage #t)
                (value (state-value cell (var-ref %r.74))))
              (ins (cached #f) (n 1)))
            (var-ref %r.74))
          (return (var-ref %r.74)))))
 (circuit %transient_hashes.17 (exported #t) (pure #f) (proof #t)
   ((%x.58 (tfield (field-native)))
     (%r.59 (tfield (field-native))))
   (tfield (field-native))
   (let* (((%h.60 (tfield (field-native))) (call
                                             %transientHash.61
                                             (tuple
                                               (single (var-ref %x.58))
                                               (single (var-ref %r.59))))))
     (let* (((%c.62 (tfield (field-native))) (call
                                               %transientCommit.63
                                               (var-ref %h.60)
                                               (var-ref %r.59))))
       (seq (public-ledger %scratch.20 (0) write (ttuple)
              (instructions
                (push (storage #f) (value (state-value cell (align 0 1))))
                (push
                  (storage #t)
                  (value (state-value cell (var-ref %c.62))))
                (ins (cached #f) (n 1)))
              (var-ref %c.62))
            (return (var-ref %c.62))))))
 (circuit %persistent_hashes.22 (exported #t) (pure #f) (proof #t)
   ((%x.64 (tbytes 32))) (tbytes 32)
   (let* (((%h.65 (tbytes 32)) (call
                                 %persistentHash.66
                                 (tuple
                                   (single
                                     '#vu8(111 112 115 58 112 104 58 0 0 0
                                           0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0
                                           0 0 0 0 0 0))
                                   (single (var-ref %x.64))))))
     (let* (((%c.67 (tbytes 32)) (call
                                   %persistentCommit.68
                                   (var-ref %h.65)
                                   (var-ref %x.64))))
       (seq (public-ledger %tag_cell.18 (1) write (ttuple)
              (instructions
                (push (storage #f) (value (state-value cell (align 1 1))))
                (push
                  (storage #t)
                  (value (state-value cell (var-ref %c.67))))
                (ins (cached #f) (n 1)))
              (var-ref %c.67))
            (return (var-ref %c.67))))))
 (circuit %transient_conversions.19 (exported #t) (pure #f)
   (proof #t) ((%x.38 (tbytes 32))) (tfield (field-native))
   (let* (((%f.39 (tfield (field-native))) (call
                                             %degradeToTransient.40
                                             (var-ref %x.38))))
     (let* (((%up.41 (tbytes 32)) (call
                                    %upgradeFromTransient.42
                                    (var-ref %f.39))))
       (let* (((%f2.43 (tfield (field-native))) (call
                                                  %degradeToTransient.40
                                                  (var-ref %up.41))))
         (seq (let* (((%tmp.44 (tfield (field-native))) (+ (tfield
                                                             (field-native))
                                                           (var-ref %f.39)
                                                           (var-ref
                                                             %f2.43))))
                (public-ledger %scratch.20 (0) write (ttuple)
                  (instructions
                    (push
                      (storage #f)
                      (value (state-value cell (align 0 1))))
                    (push
                      (storage #t)
                      (value (state-value cell (var-ref %tmp.44))))
                    (ins (cached #f) (n 1)))
                  (var-ref %tmp.44)))
              (return (var-ref %f2.43)))))))
 (circuit %curve_ops.29 (exported #t) (pure #f) (proof #t)
   ((%s.45 (tfield (field-native)))
     (%m.46 (tfield (field-native))))
   (tfield (field-native))
   (let* (((%g.47 (tpoint (curve-jubjub))) (call
                                             %ecMulGenerator.48
                                             (cast-to-field
                                               (field-scalar
                                                 (curve-jubjub))
                                               (tfield (field-native))
                                               (var-ref %s.45)))))
     (let* (((%h.49 (tpoint (curve-jubjub))) (call
                                               %hashToCurve.50
                                               (tuple
                                                 (single (var-ref %s.45))
                                                 (single
                                                   (var-ref %m.46))))))
       (let* (((%sum.51 (tpoint (curve-jubjub))) (call
                                                   %ecAdd.52
                                                   (var-ref %g.47)
                                                   (var-ref %h.49))))
         (let* (((%prod.53 (tpoint (curve-jubjub))) (call
                                                      %ecMul.54
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
                                                             %jubjubPointX.56
                                                             (var-ref
                                                               %prod.53))
                                                           (call
                                                             %jubjubPointY.57
                                                             (var-ref
                                                               %prod.53)))))
             (seq (public-ledger %scratch.20 (0) write (ttuple)
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
 (circuit %ledger_ops.25 (exported #t) (pure #f) (proof #t)
   ((%k.31 (tfield (field-native)))
     (%v.32 (tunsigned 18446744073709551615))
     (%entry.30 (tbytes 32)))
   (tboolean)
   (seq (let* (((%tmp.33 (tunsigned 65535)) (safe-cast
                                              (tunsigned 65535)
                                              (tunsigned 1)
                                              '1)))
          (public-ledger %hits.24 (4) increment (ttuple)
            (instructions
              (idx (cached #f) (pushPath #t) (path ((align 4 1))))
              (addi (immediate (value->int (var-ref %tmp.33))))
              (ins (cached #t) (n 1)))
            (var-ref %tmp.33)))
        (public-ledger %scores.23 (3) insert (ttuple)
          (instructions (idx (cached #f) (pushPath #t) (path ((align 3 1))))
            (push
              (storage #f)
              (value (state-value cell (var-ref %k.31))))
            (push
              (storage #t)
              (value (state-value ADT (var-ref %v.32))))
            (ins (cached #f) (n 1)) (ins (cached #t) (n 1)))
          (var-ref %k.31) (var-ref %v.32))
        (public-ledger %seen.21 (2) insert (ttuple)
          (instructions (idx (cached #f) (pushPath #t) (path ((align 2 1))))
            (push
              (storage #f)
              (value (state-value cell (var-ref %entry.30))))
            (push (storage #t) (value (state-value null)))
            (ins (cached #f) (n 1)) (ins (cached #t) (n 1)))
          (var-ref %entry.30))
        (return
          (public-ledger %seen.21 (2) member (tboolean)
            (instructions (dup (n 0))
              (idx (cached #f) (pushPath #f) (path ((align 2 1))))
              (push
                (storage #f)
                (value (state-value cell (var-ref %entry.30))))
              (member) (popeq (cached #t) (result (void))))
            (var-ref %entry.30)))))
 (circuit %casts.28 (exported #t) (pure #f) (proof #t)
   ((%n.34 (tunsigned 4294967295))
     (%f.35 (tfield (field-native))))
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
                                                               %n.34)))
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
     (let* (((%as_field.36 (tfield (field-native))) (safe-cast
                                                      (tfield
                                                        (field-native))
                                                      (tunsigned
                                                        36893488147419103230)
                                                      (var-ref %wide.1))))
       (let* (((%b.37 (tbytes 32)) (field->bytes
                                     32
                                     (field-native)
                                     (+ (tfield (field-native))
                                        (var-ref %f.35)
                                        (var-ref %as_field.36)))))
         (seq (public-ledger %tag_cell.18 (1) write (ttuple)
                (instructions
                  (push
                    (storage #f)
                    (value (state-value cell (align 1 1))))
                  (push
                    (storage #t)
                    (value (state-value cell (var-ref %b.37))))
                  (ins (cached #f) (n 1)))
                (var-ref %b.37))
              (return (var-ref %b.37))))))))
