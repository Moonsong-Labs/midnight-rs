(normalized-ir (compiler-version "0.33.122") (language-version "0.25.107")
  (runtime-version "0.18.107")
  (exports (digest . %digest.52) (fill_slots . %fill_slots.53)
    (fold_shift . %fold_shift.50)
    (fold_shift_named . %fold_shift_named.51)
    (map_scale . %map_scale.48)
    (map_then_fold . %map_then_fold.49) (rounds . %rounds.46)
    (slots . %slots.47) (total . %total.45))
  (contract-types)
  (kernel-declaration (%kernel.79 () (exported #f) (Kernel)))
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
  (native
    %transientHash.71
    (entry "__compactRuntime.transientHash" circuit)
    ((%value.76 (tvector 4 (tfield (field-native)))))
    (tfield (field-native)))
  (circuit %shift_in.62 (exported #f) (pure #t) (proof #f)
    ((%acc.77 (tfield (field-native)))
      (%x.78 (tfield (field-native))))
    (tfield (field-native))
    (return
      (+ (tfield (field-native))
         (* (tfield (field-native))
            (var-ref %acc.77)
            (safe-cast (tfield (field-native)) (tunsigned 3) '3))
         (var-ref %x.78))))
  (circuit %map_scale.48 (exported #t) (pure #f) (proof #t)
    ((%xs.67 (tvector 4 (tfield (field-native)))))
    (tfield (field-native))
    (let* (((%scaled.68 (tvector 4 (tfield (field-native)))) (map 4
                                                                  (circuit
                                                                    ((%x.69
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
                                                                              %x.69)
                                                                            (var-ref
                                                                              %x.69))
                                                                         (safe-cast
                                                                           (tfield
                                                                             (field-native))
                                                                           (tunsigned
                                                                             1)
                                                                           '1))))
                                                                  ((var-ref
                                                                     %xs.67)
                                                                    (tvector
                                                                      4
                                                                      (tfield
                                                                        (field-native)))
                                                                    (tfield
                                                                      (field-native))))))
      (let* (((%h.70 (tfield (field-native))) (call
                                                %transientHash.71
                                                (var-ref %scaled.68))))
        (seq (public-ledger %digest.52 (0) write (ttuple)
               (instructions
                 (push (storage #f) (value (state-value cell (align 0 1))))
                 (push
                   (storage #t)
                   (value (state-value cell (var-ref %h.70))))
                 (ins (cached #f) (n 1)))
               (var-ref %h.70))
             (return (var-ref %h.70))))))
  (circuit %fold_shift.50 (exported #t) (pure #f) (proof #t)
    ((%xs.72 (tvector 4 (tfield (field-native)))))
    (tfield (field-native))
    (let* (((%acc.73 (tfield (field-native))) (fold
                                                4
                                                (circuit
                                                  ((%a.74
                                                     (tfield
                                                       (field-native)))
                                                    (%x.75
                                                      (tfield
                                                        (field-native))))
                                                  (tfield (field-native))
                                                  (return
                                                    (+ (tfield
                                                         (field-native))
                                                       (* (tfield
                                                            (field-native))
                                                          (var-ref %a.74)
                                                          (safe-cast
                                                            (tfield
                                                              (field-native))
                                                            (tunsigned 3)
                                                            '3))
                                                       (var-ref %x.75))))
                                                ((safe-cast
                                                   (tfield (field-native))
                                                   (tunsigned 0)
                                                   '0)
                                                  (tfield (field-native)))
                                                ((var-ref %xs.72)
                                                  (tvector
                                                    4
                                                    (tfield
                                                      (field-native)))
                                                  (tfield
                                                    (field-native))))))
      (seq (public-ledger %total.45 (1) write (ttuple)
             (instructions
               (push (storage #f) (value (state-value cell (align 1 1))))
               (push
                 (storage #t)
                 (value (state-value cell (var-ref %acc.73))))
               (ins (cached #f) (n 1)))
             (var-ref %acc.73))
           (return (var-ref %acc.73)))))
  (circuit %fold_shift_named.51 (exported #t) (pure #f) (proof #t)
    ((%xs.60 (tvector 4 (tfield (field-native)))))
    (tfield (field-native))
    (let* (((%acc.61 (tfield (field-native))) (fold
                                                4
                                                (fref %shift_in.62)
                                                ((safe-cast
                                                   (tfield (field-native))
                                                   (tunsigned 0)
                                                   '0)
                                                  (tfield (field-native)))
                                                ((var-ref %xs.60)
                                                  (tvector
                                                    4
                                                    (tfield
                                                      (field-native)))
                                                  (tfield
                                                    (field-native))))))
      (seq (public-ledger %total.45 (1) write (ttuple)
             (instructions
               (push (storage #f) (value (state-value cell (align 1 1))))
               (push
                 (storage #t)
                 (value (state-value cell (var-ref %acc.61))))
               (ins (cached #f) (n 1)))
             (var-ref %acc.61))
           (return (var-ref %acc.61)))))
  (circuit %map_then_fold.49 (exported #t) (pure #f) (proof #t)
    ((%xs.63 (tvector 4 (tfield (field-native)))))
    (tfield (field-native))
    (let* (((%scaled.64 (tvector 4 (tfield (field-native)))) (map 4
                                                                  (circuit
                                                                    ((%x.65
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
                                                                              %x.65)
                                                                            (var-ref
                                                                              %x.65))
                                                                         (safe-cast
                                                                           (tfield
                                                                             (field-native))
                                                                           (tunsigned
                                                                             1)
                                                                           '1))))
                                                                  ((var-ref
                                                                     %xs.63)
                                                                    (tvector
                                                                      4
                                                                      (tfield
                                                                        (field-native)))
                                                                    (tfield
                                                                      (field-native))))))
      (let* (((%acc.66 (tfield (field-native))) (fold
                                                  4
                                                  (fref %shift_in.62)
                                                  ((safe-cast
                                                     (tfield
                                                       (field-native))
                                                     (tunsigned 1)
                                                     '1)
                                                    (tfield
                                                      (field-native)))
                                                  ((var-ref %scaled.64)
                                                    (tvector
                                                      4
                                                      (tfield
                                                        (field-native)))
                                                    (tfield
                                                      (field-native))))))
        (seq (public-ledger %total.45 (1) write (ttuple)
               (instructions
                 (push (storage #f) (value (state-value cell (align 1 1))))
                 (push
                   (storage #t)
                   (value (state-value cell (var-ref %acc.66))))
                 (ins (cached #f) (n 1)))
               (var-ref %acc.66))
             (return (var-ref %acc.66))))))
  (circuit %fill_slots.53 (exported #t) (pure #f) (proof #t)
    ((%base.54 (tunsigned 255))) (ttuple)
    (seq (fold
           4
           (circuit
             ((%t.55 (ttuple)) (%i.56 (tunsigned 3)))
             (ttuple)
             (seq (seq (let* (((%tmp.57 (tunsigned 65535)) (safe-cast
                                                             (tunsigned
                                                               65535)
                                                             (tunsigned 1)
                                                             '1)))
                         (public-ledger %rounds.46 (2) increment (ttuple)
                           (instructions
                             (idx (cached #f)
                                  (pushPath #t)
                                  (path ((align 2 1))))
                             (addi
                               (immediate (value->int (var-ref %tmp.57))))
                             (ins (cached #t) (n 1)))
                           (var-ref %tmp.57)))
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
                                                                                       %base.54)))
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
                           (public-ledger %slots.47 (3) insert (ttuple)
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
                                   (state-value ADT (var-ref %tmp.59))))
                               (ins (cached #f) (n 1))
                               (ins (cached #t) (n 1)))
                             (var-ref %tmp.58) (var-ref %tmp.59))))
                       (tuple))
                  (var-ref %t.55)))
           ((tuple) (ttuple))
           ((tuple (single '0) (single '1) (single '2) (single '3))
             (ttuple
               (tunsigned 0)
               (tunsigned 1)
               (tunsigned 2)
               (tunsigned 3))
             (tunsigned 3)))
         (return (tuple)))))
