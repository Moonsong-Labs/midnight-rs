(analyzed-ir (compiler-version "0.33.122") (language-version "0.25.107")
  (runtime-version "0.18.107")
  (exports (digest . %digest.52) (fill_slots . %fill_slots.53)
    (fold_shift . %fold_shift.50)
    (fold_shift_named . %fold_shift_named.51)
    (map_scale . %map_scale.48)
    (map_then_fold . %map_then_fold.49) (rounds . %rounds.46)
    (slots . %slots.47) (total . %total.45))
  (contract-types)
  (kernel-declaration (%kernel.78 () (exported #f) (Kernel)))
  (public-ledger-declaration
    (public-ledger-array
      (%digest.52
        (0)
        (exported #t)
        (__compact_Cell (tfield (field-native))))
      (%total.45
        (1)
        (exported #t)
        (__compact_Cell (tfield (field-native))))
      (%rounds.46 (2) (exported #t) (Counter))
      (%slots.47
        (3)
        (exported #t)
        (Map (tunsigned 255) (tunsigned 18446744073709551615))))
    (constructor () (tuple)))
  (native %transientHash.69
    (entry "__compactRuntime.transientHash" circuit)
    (type-arguments (tvector 4 (tfield (field-native))))
    ((%value.79 (tvector 4 (tfield (field-native)))))
    (tfield (field-native)))
  (circuit %shift_in.60 (exported #f) (pure #t) (proof #f)
    ((%acc.76 (tfield (field-native)))
      (%x.77 (tfield (field-native))))
    (tfield (field-native))
    (return
      (+ (tfield (field-native))
         (* (tfield (field-native))
            (var-ref %acc.76)
            (safe-cast (tfield (field-native)) (tunsigned 3) '3))
         (var-ref %x.77))))
  (circuit %map_scale.48 (exported #t) (pure #f) (proof #t)
    ((%xs.68 (tvector 4 (tfield (field-native)))))
    (tfield (field-native))
    (let* (((%scaled.70 (tvector 4 (tfield (field-native)))) (map 4
                                                                  (circuit
                                                                    ((%x.67
                                                                       (tfield
                                                                         (field-native))))
                                                                    (tfield
                                                                      (field-native))
                                                                    (return
                                                                      (+ (tfield
                                                                           (field-native))
                                                                         (+ (tfield
                                                                              (field-native))
                                                                            (var-ref
                                                                              %x.67)
                                                                            (var-ref
                                                                              %x.67))
                                                                         (safe-cast
                                                                           (tfield
                                                                             (field-native))
                                                                           (tunsigned
                                                                             1)
                                                                           '1))))
                                                                  ((var-ref
                                                                     %xs.68)
                                                                    (tvector
                                                                      4
                                                                      (tfield
                                                                        (field-native)))
                                                                    (tfield
                                                                      (field-native))))))
      (let* (((%h.71 (tfield (field-native))) (call
                                                %transientHash.69
                                                (var-ref %scaled.70))))
        (seq (public-ledger %digest.52 write (0) write (ttuple)
               (instructions
                 (push (storage #f) (value (state-value cell (align 0 1))))
                 (push
                   (storage #t)
                   (value (state-value cell (var-ref %h.71))))
                 (ins (cached #f) (n 1)))
               (var-ref %h.71))
             (return (var-ref %h.71))))))
  (circuit %fold_shift.50 (exported #t) (pure #f) (proof #t)
    ((%xs.74 (tvector 4 (tfield (field-native)))))
    (tfield (field-native))
    (let* (((%acc.75 (tfield (field-native))) (fold
                                                4
                                                (circuit
                                                  ((%a.72
                                                     (tfield
                                                       (field-native)))
                                                    (%x.73
                                                      (tfield
                                                        (field-native))))
                                                  (tfield (field-native))
                                                  (return
                                                    (+ (tfield
                                                         (field-native))
                                                       (* (tfield
                                                            (field-native))
                                                          (var-ref %a.72)
                                                          (safe-cast
                                                            (tfield
                                                              (field-native))
                                                            (tunsigned 3)
                                                            '3))
                                                       (var-ref %x.73))))
                                                ((safe-cast
                                                   (tfield (field-native))
                                                   (tunsigned 0)
                                                   '0)
                                                  (tfield (field-native)))
                                                ((var-ref %xs.74)
                                                  (tvector
                                                    4
                                                    (tfield
                                                      (field-native)))
                                                  (tfield
                                                    (field-native))))))
      (seq (public-ledger %total.45 write (1) write (ttuple)
             (instructions
               (push (storage #f) (value (state-value cell (align 1 1))))
               (push
                 (storage #t)
                 (value (state-value cell (var-ref %acc.75))))
               (ins (cached #f) (n 1)))
             (var-ref %acc.75))
           (return (var-ref %acc.75)))))
  (circuit %fold_shift_named.51 (exported #t) (pure #f) (proof #t)
    ((%xs.61 (tvector 4 (tfield (field-native)))))
    (tfield (field-native))
    (let* (((%acc.62 (tfield (field-native))) (fold
                                                4
                                                (fref %shift_in.60)
                                                ((safe-cast
                                                   (tfield (field-native))
                                                   (tunsigned 0)
                                                   '0)
                                                  (tfield (field-native)))
                                                ((var-ref %xs.61)
                                                  (tvector
                                                    4
                                                    (tfield
                                                      (field-native)))
                                                  (tfield
                                                    (field-native))))))
      (seq (public-ledger %total.45 write (1) write (ttuple)
             (instructions
               (push (storage #f) (value (state-value cell (align 1 1))))
               (push
                 (storage #t)
                 (value (state-value cell (var-ref %acc.62))))
               (ins (cached #f) (n 1)))
             (var-ref %acc.62))
           (return (var-ref %acc.62)))))
  (circuit %map_then_fold.49 (exported #t) (pure #f) (proof #t)
    ((%xs.64 (tvector 4 (tfield (field-native)))))
    (tfield (field-native))
    (let* (((%scaled.65 (tvector 4 (tfield (field-native)))) (map 4
                                                                  (circuit
                                                                    ((%x.63
                                                                       (tfield
                                                                         (field-native))))
                                                                    (tfield
                                                                      (field-native))
                                                                    (return
                                                                      (+ (tfield
                                                                           (field-native))
                                                                         (+ (tfield
                                                                              (field-native))
                                                                            (var-ref
                                                                              %x.63)
                                                                            (var-ref
                                                                              %x.63))
                                                                         (safe-cast
                                                                           (tfield
                                                                             (field-native))
                                                                           (tunsigned
                                                                             1)
                                                                           '1))))
                                                                  ((var-ref
                                                                     %xs.64)
                                                                    (tvector
                                                                      4
                                                                      (tfield
                                                                        (field-native)))
                                                                    (tfield
                                                                      (field-native))))))
      (let* (((%acc.66 (tfield (field-native))) (fold
                                                  4
                                                  (fref %shift_in.60)
                                                  ((safe-cast
                                                     (tfield
                                                       (field-native))
                                                     (tunsigned 1)
                                                     '1)
                                                    (tfield
                                                      (field-native)))
                                                  ((var-ref %scaled.65)
                                                    (tvector
                                                      4
                                                      (tfield
                                                        (field-native)))
                                                    (tfield
                                                      (field-native))))))
        (seq (public-ledger %total.45 write (1) write (ttuple)
               (instructions
                 (push (storage #f) (value (state-value cell (align 1 1))))
                 (push
                   (storage #t)
                   (value (state-value cell (var-ref %acc.66))))
                 (ins (cached #f) (n 1)))
               (var-ref %acc.66))
             (return (var-ref %acc.66))))))
  (circuit %fill_slots.53 (exported #t) (pure #f) (proof #t)
    ((%base.57 (tunsigned 255))) (ttuple)
    (seq (fold
           4
           (circuit
             ((%t.54 (ttuple)) (%i.56 (tunsigned 3)))
             (ttuple)
             (seq (seq (let* (((%tmp.55 (tunsigned 65535)) (safe-cast
                                                             (tunsigned
                                                               65535)
                                                             (tunsigned 1)
                                                             '1)))
                         (public-ledger %rounds.46 update (2) increment (ttuple)
                           (instructions
                             (idx (cached #f)
                                  (pushPath #t)
                                  (path ((align 2 1))))
                             (addi
                               (immediate (value->int (var-ref %tmp.55))))
                             (ins (cached #t) (n 1)))
                           (var-ref %tmp.55)))
                       (let* (((%tmp.58 (tunsigned 255)) (safe-cast
                                                           (tunsigned 255)
                                                           (tunsigned 3)
                                                           (var-ref
                                                             %i.56))))
                         (let* (((%tmp.59 (tunsigned 18446744073709551615)) (safe-cast
                                                                              (tunsigned
                                                                                18446744073709551615)
                                                                              (tunsigned
                                                                                131070)
                                                                              (+ (tunsigned
                                                                                   131070)
                                                                                 (safe-cast
                                                                                   (tunsigned
                                                                                     131070)
                                                                                   (tunsigned
                                                                                     65535)
                                                                                   (safe-cast
                                                                                     (tunsigned
                                                                                       65535)
                                                                                     (tunsigned
                                                                                       255)
                                                                                     (var-ref
                                                                                       %base.57)))
                                                                                 (safe-cast
                                                                                   (tunsigned
                                                                                     131070)
                                                                                   (tunsigned
                                                                                     65535)
                                                                                   (safe-cast
                                                                                     (tunsigned
                                                                                       65535)
                                                                                     (tunsigned
                                                                                       3)
                                                                                     (var-ref
                                                                                       %i.56)))))))
                           (public-ledger %slots.47 update (3) insert (ttuple)
                             (instructions
                               (idx (cached #f)
                                    (pushPath #t)
                                    (path ((align 3 1))))
                               (push
                                 (storage #f)
                                 (value
                                   (state-value cell (var-ref %tmp.58))))
                               (push
                                 (storage #t)
                                 (value
                                   (state-value
                                     ADT
                                     (var-ref %tmp.59)
                                     (tunsigned 18446744073709551615))))
                               (ins (cached #f) (n 1))
                               (ins (cached #t) (n 1)))
                             (var-ref %tmp.58) (var-ref %tmp.59))))
                       (tuple))
                  (var-ref %t.54)))
           ((tuple) (ttuple))
           ((tuple (single '0) (single '1) (single '2) (single '3))
             (ttuple
               (tunsigned 0)
               (tunsigned 1)
               (tunsigned 2)
               (tunsigned 3))
             (tunsigned 3)))
         (return (tuple)))))
