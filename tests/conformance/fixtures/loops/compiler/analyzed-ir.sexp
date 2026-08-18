(analyzed-ir (compiler-version "0.33.122") (language-version "0.25.107")
  (runtime-version "0.18.107")
  (exports (digest . %digest.7) (fill_slots . %fill_slots.8)
    (fold_shift . %fold_shift.5)
    (fold_shift_named . %fold_shift_named.6)
    (map_scale . %map_scale.3)
    (map_then_fold . %map_then_fold.4) (rounds . %rounds.1)
    (slots . %slots.2) (total . %total.0))
  (contract-types)
  (kernel-declaration (%kernel.34 () (exported #f) (Kernel)))
  (public-ledger-declaration
    (public-ledger-array
      (%digest.7
        (0)
        (exported #t)
        (__compact_Cell (tfield (field-native))))
      (%total.0
        (1)
        (exported #t)
        (__compact_Cell (tfield (field-native))))
      (%rounds.1 (2) (exported #t) (Counter))
      (%slots.2
        (3)
        (exported #t)
        (Map (tunsigned 255) (tunsigned 18446744073709551615))))
    (constructor () (tuple)))
  (native %transientHash.26
    (entry "__compactRuntime.transientHash" circuit)
    (type-arguments (tvector 4 (tfield (field-native))))
    ((%value.31 (tvector 4 (tfield (field-native)))))
    (tfield (field-native)))
  (circuit %shift_in.17 (exported #f) (pure #t) (proof #f)
    ((%acc.32 (tfield (field-native)))
      (%x.33 (tfield (field-native))))
    (tfield (field-native))
    (return
      (+ (tfield (field-native))
         (* (tfield (field-native))
            (var-ref %acc.32)
            (safe-cast (tfield (field-native)) (tunsigned 3) '3))
         (var-ref %x.33))))
  (circuit %map_scale.3 (exported #t) (pure #f) (proof #t)
    ((%xs.22 (tvector 4 (tfield (field-native)))))
    (tfield (field-native))
    (let* (((%scaled.23 (tvector 4 (tfield (field-native)))) (map 4
                                                                  (circuit
                                                                    ((%x.24
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
                                                                              %x.24)
                                                                            (var-ref
                                                                              %x.24))
                                                                         (safe-cast
                                                                           (tfield
                                                                             (field-native))
                                                                           (tunsigned
                                                                             1)
                                                                           '1))))
                                                                  ((var-ref
                                                                     %xs.22)
                                                                    (tvector
                                                                      4
                                                                      (tfield
                                                                        (field-native)))
                                                                    (tfield
                                                                      (field-native))))))
      (let* (((%h.25 (tfield (field-native))) (call
                                                %transientHash.26
                                                (var-ref %scaled.23))))
        (seq (public-ledger %digest.7 write (0) write (ttuple)
               (instructions
                 (push (storage #f) (value (state-value cell (align 0 1))))
                 (push
                   (storage #t)
                   (value (state-value cell (var-ref %h.25))))
                 (ins (cached #f) (n 1)))
               (var-ref %h.25))
             (return (var-ref %h.25))))))
  (circuit %fold_shift.5 (exported #t) (pure #f) (proof #t)
    ((%xs.27 (tvector 4 (tfield (field-native)))))
    (tfield (field-native))
    (let* (((%acc.28 (tfield (field-native))) (fold
                                                4
                                                (circuit
                                                  ((%a.29
                                                     (tfield
                                                       (field-native)))
                                                    (%x.30
                                                      (tfield
                                                        (field-native))))
                                                  (tfield (field-native))
                                                  (return
                                                    (+ (tfield
                                                         (field-native))
                                                       (* (tfield
                                                            (field-native))
                                                          (var-ref %a.29)
                                                          (safe-cast
                                                            (tfield
                                                              (field-native))
                                                            (tunsigned 3)
                                                            '3))
                                                       (var-ref %x.30))))
                                                ((safe-cast
                                                   (tfield (field-native))
                                                   (tunsigned 0)
                                                   '0)
                                                  (tfield (field-native)))
                                                ((var-ref %xs.27)
                                                  (tvector
                                                    4
                                                    (tfield
                                                      (field-native)))
                                                  (tfield
                                                    (field-native))))))
      (seq (public-ledger %total.0 write (1) write (ttuple)
             (instructions
               (push (storage #f) (value (state-value cell (align 1 1))))
               (push
                 (storage #t)
                 (value (state-value cell (var-ref %acc.28))))
               (ins (cached #f) (n 1)))
             (var-ref %acc.28))
           (return (var-ref %acc.28)))))
  (circuit %fold_shift_named.6 (exported #t) (pure #f) (proof #t)
    ((%xs.15 (tvector 4 (tfield (field-native)))))
    (tfield (field-native))
    (let* (((%acc.16 (tfield (field-native))) (fold
                                                4
                                                (fref %shift_in.17)
                                                ((safe-cast
                                                   (tfield (field-native))
                                                   (tunsigned 0)
                                                   '0)
                                                  (tfield (field-native)))
                                                ((var-ref %xs.15)
                                                  (tvector
                                                    4
                                                    (tfield
                                                      (field-native)))
                                                  (tfield
                                                    (field-native))))))
      (seq (public-ledger %total.0 write (1) write (ttuple)
             (instructions
               (push (storage #f) (value (state-value cell (align 1 1))))
               (push
                 (storage #t)
                 (value (state-value cell (var-ref %acc.16))))
               (ins (cached #f) (n 1)))
             (var-ref %acc.16))
           (return (var-ref %acc.16)))))
  (circuit %map_then_fold.4 (exported #t) (pure #f) (proof #t)
    ((%xs.18 (tvector 4 (tfield (field-native)))))
    (tfield (field-native))
    (let* (((%scaled.19 (tvector 4 (tfield (field-native)))) (map 4
                                                                  (circuit
                                                                    ((%x.20
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
                                                                              %x.20)
                                                                            (var-ref
                                                                              %x.20))
                                                                         (safe-cast
                                                                           (tfield
                                                                             (field-native))
                                                                           (tunsigned
                                                                             1)
                                                                           '1))))
                                                                  ((var-ref
                                                                     %xs.18)
                                                                    (tvector
                                                                      4
                                                                      (tfield
                                                                        (field-native)))
                                                                    (tfield
                                                                      (field-native))))))
      (let* (((%acc.21 (tfield (field-native))) (fold
                                                  4
                                                  (fref %shift_in.17)
                                                  ((safe-cast
                                                     (tfield
                                                       (field-native))
                                                     (tunsigned 1)
                                                     '1)
                                                    (tfield
                                                      (field-native)))
                                                  ((var-ref %scaled.19)
                                                    (tvector
                                                      4
                                                      (tfield
                                                        (field-native)))
                                                    (tfield
                                                      (field-native))))))
        (seq (public-ledger %total.0 write (1) write (ttuple)
               (instructions
                 (push (storage #f) (value (state-value cell (align 1 1))))
                 (push
                   (storage #t)
                   (value (state-value cell (var-ref %acc.21))))
                 (ins (cached #f) (n 1)))
               (var-ref %acc.21))
             (return (var-ref %acc.21))))))
  (circuit %fill_slots.8 (exported #t) (pure #f) (proof #t)
    ((%base.9 (tunsigned 255))) (ttuple)
    (seq (fold
           4
           (circuit
             ((%t.10 (ttuple)) (%i.11 (tunsigned 3)))
             (ttuple)
             (seq (seq (let* (((%tmp.12 (tunsigned 65535)) (safe-cast
                                                             (tunsigned
                                                               65535)
                                                             (tunsigned 1)
                                                             '1)))
                         (public-ledger %rounds.1 update (2) increment (ttuple)
                           (instructions
                             (idx (cached #f)
                                  (pushPath #t)
                                  (path ((align 2 1))))
                             (addi
                               (immediate (value->int (var-ref %tmp.12))))
                             (ins (cached #t) (n 1)))
                           (var-ref %tmp.12)))
                       (let* (((%tmp.13 (tunsigned 255)) (safe-cast
                                                           (tunsigned 255)
                                                           (tunsigned 3)
                                                           (var-ref
                                                             %i.11))))
                         (let* (((%tmp.14 (tunsigned 18446744073709551615)) (safe-cast
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
                                                                                       %base.9)))
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
                                                                                       %i.11)))))))
                           (public-ledger %slots.2 update (3) insert (ttuple)
                             (instructions
                               (idx (cached #f)
                                    (pushPath #t)
                                    (path ((align 3 1))))
                               (push
                                 (storage #f)
                                 (value
                                   (state-value cell (var-ref %tmp.13))))
                               (push
                                 (storage #t)
                                 (value
                                   (state-value
                                     ADT
                                     (var-ref %tmp.14)
                                     (tunsigned 18446744073709551615))))
                               (ins (cached #f) (n 1))
                               (ins (cached #t) (n 1)))
                             (var-ref %tmp.13) (var-ref %tmp.14))))
                       (tuple))
                  (var-ref %t.10)))
           ((tuple) (ttuple))
           ((tuple (single '0) (single '1) (single '2) (single '3))
             (ttuple
               (tunsigned 0)
               (tunsigned 1)
               (tunsigned 2)
               (tunsigned 3))
             (tunsigned 3)))
         (return (tuple)))))
