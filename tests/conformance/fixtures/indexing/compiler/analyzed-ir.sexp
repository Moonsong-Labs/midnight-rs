(analyzed-ir (compiler-version "0.33.122") (language-version "0.25.107")
  (runtime-version "0.18.107")
  (exports (byte_sum . %byte_sum.22) (copy_members . %copy_members.23)
    (first_key . %first_key.20) (found_key . %found_key.21)
    (ids . %ids.18) (members . %members.19) (row . %row.16)
    (rows . %rows.17) (seed . %seed.14)
    (store_slice . %store_slice.15) (sum_bytes . %sum_bytes.12)
    (tail_id . %tail_id.13) (tail_of_row . %tail_of_row.11))
  (contract-types)
  (kernel-declaration (%kernel.42 () (exported #f) (Kernel)))
  (public-ledger-declaration
    (public-ledger-array
      (%members.19
        (0)
        (exported #t)
        (__compact_Cell
          (tvector
            3
            (tstruct Member (id (tunsigned 65535)) (key (tbytes 4))))))
      (%row.16
        (1)
        (exported #t)
        (__compact_Cell
          (ttuple
            (tstruct Member (id (tunsigned 65535)) (key (tbytes 4)))
            (tunsigned 65535))))
      (%ids.18
        (2)
        (exported #t)
        (Map (tunsigned 255) (tunsigned 65535)))
      (%found_key.21
        (3)
        (exported #t)
        (__compact_Cell (tbytes 4)))
      (%tail_id.13
        (4)
        (exported #t)
        (__compact_Cell (tunsigned 65535)))
      (%byte_sum.22
        (5)
        (exported #t)
        (__compact_Cell (tunsigned 4294967295)))
      (%rows.17
        (6)
        (exported #t)
        (Map (tunsigned 255) (tvector 3 (tfield (field-native))))))
    (constructor () (tuple)))
  (export-typedef
    Member
    ()
    (tstruct Member (id (tunsigned 65535)) (key (tbytes 4))))
  (circuit %seed.14 (exported #t) (pure #f) (proof #t)
    ((%ms.38
       (tvector
         3
         (tstruct Member (id (tunsigned 65535)) (key (tbytes 4)))))
      (%m.39
        (tstruct Member (id (tunsigned 65535)) (key (tbytes 4))))
      (%n.40 (tunsigned 65535)))
    (ttuple)
    (seq (public-ledger %members.19 write (0) write (ttuple)
           (instructions
             (push (storage #f) (value (state-value cell (align 0 1))))
             (push
               (storage #t)
               (value (state-value cell (var-ref %ms.38))))
             (ins (cached #f) (n 1)))
           (var-ref %ms.38))
         (let* (((%tmp.41
                   (ttuple
                     (tstruct
                       Member
                       (id (tunsigned 65535))
                       (key (tbytes 4)))
                     (tunsigned 65535))) (tuple
                                           (single (var-ref %m.39))
                                           (single (var-ref %n.40)))))
           (public-ledger %row.16 write (1) write (ttuple)
             (instructions
               (push (storage #f) (value (state-value cell (align 1 1))))
               (push
                 (storage #t)
                 (value (state-value cell (var-ref %tmp.41))))
               (ins (cached #f) (n 1)))
             (var-ref %tmp.41)))
         (return (tuple))))
  (circuit %copy_members.23 (exported #t) (pure #f) (proof #t) ()
    (ttuple)
    (seq (fold
           3
           (circuit
             ((%t.33 (ttuple)) (%i.34 (tunsigned 2)))
             (ttuple)
             (seq (seq (let* (((%tmp.35 (tunsigned 255)) (safe-cast
                                                           (tunsigned 255)
                                                           (tunsigned 2)
                                                           (var-ref
                                                             %i.34))))
                         (let* (((%tmp.36 (tunsigned 65535)) (elt-ref
                                                               (vector-ref
                                                                 (tvector
                                                                   3
                                                                   (tstruct
                                                                     Member
                                                                     (id (tunsigned
                                                                           65535))
                                                                     (key (tbytes
                                                                            4))))
                                                                 (public-ledger
                                                                   %members.19
                                                                   read (0)
                                                                   read
                                                                   (tvector
                                                                     3
                                                                     (tstruct
                                                                       Member
                                                                       (id (tunsigned
                                                                             65535))
                                                                       (key (tbytes
                                                                              4))))
                                                                   (instructions
                                                                     (dup (n 0))
                                                                     (idx (cached
                                                                            #f)
                                                                          (pushPath
                                                                            #f)
                                                                          (path
                                                                            ((align
                                                                               0
                                                                               1))))
                                                                     (popeq
                                                                       (cached
                                                                         #f)
                                                                       (result
                                                                         (void)))))
                                                                 (var-ref
                                                                   %i.34))
                                                               id
                                                               0)))
                           (public-ledger %ids.18 update (2) insert (ttuple)
                             (instructions
                               (idx (cached #f)
                                    (pushPath #t)
                                    (path ((align 2 1))))
                               (push
                                 (storage #f)
                                 (value
                                   (state-value cell (var-ref %tmp.35))))
                               (push
                                 (storage #t)
                                 (value
                                   (state-value
                                     ADT
                                     (var-ref %tmp.36)
                                     (tunsigned 65535))))
                               (ins (cached #f) (n 1))
                               (ins (cached #t) (n 1)))
                             (var-ref %tmp.35) (var-ref %tmp.36))))
                       (tuple))
                  (var-ref %t.33)))
           ((tuple) (ttuple))
           ((tuple (single '0) (single '1) (single '2))
             (ttuple (tunsigned 0) (tunsigned 1) (tunsigned 2))
             (tunsigned 2)))
         (return (tuple))))
  (circuit %first_key.20 (exported #t) (pure #f) (proof #t) ()
    (tbytes 4)
    (let* (((%k.37 (tbytes 4)) (elt-ref
                                 (tuple-ref
                                   (public-ledger %members.19 read (0) read
                                     (tvector
                                       3
                                       (tstruct
                                         Member
                                         (id (tunsigned 65535))
                                         (key (tbytes 4))))
                                     (instructions
                                       (dup (n 0))
                                       (idx (cached #f)
                                            (pushPath #f)
                                            (path ((align 0 1))))
                                       (popeq
                                         (cached #f)
                                         (result (void)))))
                                   0)
                                 key
                                 1)))
      (seq (public-ledger %found_key.21 write (3) write (ttuple)
             (instructions
               (push (storage #f) (value (state-value cell (align 3 1))))
               (push
                 (storage #t)
                 (value (state-value cell (var-ref %k.37))))
               (ins (cached #f) (n 1)))
             (var-ref %k.37))
           (return (var-ref %k.37)))))
  (circuit %tail_of_row.11 (exported #t) (pure #f) (proof #t) ()
    (tunsigned 65535)
    (let* (((%tail.27 (ttuple (tunsigned 65535))) (tuple-slice
                                                    (ttuple
                                                      (tstruct
                                                        Member
                                                        (id (tunsigned
                                                              65535))
                                                        (key (tbytes 4)))
                                                      (tunsigned 65535))
                                                    (public-ledger %row.16 read (1) read
                                                      (ttuple
                                                        (tstruct
                                                          Member
                                                          (id (tunsigned
                                                                65535))
                                                          (key (tbytes 4)))
                                                        (tunsigned 65535))
                                                      (instructions
                                                        (dup (n 0))
                                                        (idx (cached #f)
                                                             (pushPath #f)
                                                             (path
                                                               ((align
                                                                  1
                                                                  1))))
                                                        (popeq
                                                          (cached #f)
                                                          (result
                                                            (void)))))
                                                    1
                                                    1)))
      (seq (let* (((%tmp.28 (tunsigned 65535)) (tuple-ref
                                                 (var-ref %tail.27)
                                                 0)))
             (public-ledger %tail_id.13 write (4) write (ttuple)
               (instructions
                 (push (storage #f) (value (state-value cell (align 4 1))))
                 (push
                   (storage #t)
                   (value (state-value cell (var-ref %tmp.28))))
                 (ins (cached #f) (n 1)))
               (var-ref %tmp.28)))
           (return (tuple-ref (var-ref %tail.27) 0)))))
  (circuit %sum_bytes.12 (exported #t) (pure #f) (proof #t)
    ((%b.31 (tbytes 4))) (tunsigned 4294967295)
    (let* (((%total.32 (tunsigned 4294967295)) (fold
                                                 4
                                                 (circuit
                                                   ((%a.29
                                                      (tunsigned
                                                        4294967295))
                                                     (%x.30
                                                       (tunsigned 255)))
                                                   (tunsigned 4294967295)
                                                   (return
                                                     (downcast-unsigned
                                                       8589934590
                                                       4294967295
                                                       (+ (tunsigned
                                                            8589934590)
                                                          (safe-cast
                                                            (tunsigned
                                                              8589934590)
                                                            (tunsigned
                                                              4294967295)
                                                            (var-ref
                                                              %a.29))
                                                          (safe-cast
                                                            (tunsigned
                                                              8589934590)
                                                            (tunsigned
                                                              4294967295)
                                                            (safe-cast
                                                              (tunsigned
                                                                4294967295)
                                                              (tunsigned
                                                                255)
                                                              (var-ref
                                                                %x.30)))))))
                                                 ((safe-cast
                                                    (tunsigned 4294967295)
                                                    (tunsigned 0)
                                                    '0)
                                                   (tunsigned 4294967295))
                                                 ((var-ref %b.31)
                                                   (tbytes 4)
                                                   (tunsigned 255)))))
      (seq (public-ledger %byte_sum.22 write (5) write (ttuple)
             (instructions
               (push (storage #f) (value (state-value cell (align 5 1))))
               (push
                 (storage #t)
                 (value (state-value cell (var-ref %total.32))))
               (ins (cached #f) (n 1)))
             (var-ref %total.32))
           (return (var-ref %total.32)))))
  (circuit %store_slice.15 (exported #t) (pure #f) (proof #t)
    ((%xs.24 (tvector 6 (tfield (field-native))))) (ttuple)
    (seq (let* (((%mid.26 (tvector 3 (tfield (field-native)))) (tuple-slice
                                                                 (tvector
                                                                   6
                                                                   (tfield
                                                                     (field-native)))
                                                                 (var-ref
                                                                   %xs.24)
                                                                 2
                                                                 3)))
           (let* (((%tmp.25 (tunsigned 255)) (safe-cast
                                               (tunsigned 255)
                                               (tunsigned 0)
                                               '0)))
             (public-ledger %rows.17 update (6) insert (ttuple)
               (instructions (idx (cached #f) (pushPath #t) (path ((align 6 1))))
                 (push
                   (storage #f)
                   (value (state-value cell (var-ref %tmp.25))))
                 (push
                   (storage #t)
                   (value
                     (state-value
                       ADT
                       (var-ref %mid.26)
                       (tvector 3 (tfield (field-native))))))
                 (ins (cached #f) (n 1)) (ins (cached #t) (n 1)))
               (var-ref %tmp.25) (var-ref %mid.26))))
         (return (tuple)))))
