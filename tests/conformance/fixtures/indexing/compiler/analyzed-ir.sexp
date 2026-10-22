(analyzed-ir (compiler-version "0.35.103-dev")
  (language-version "0.27.0") (runtime-version "0.20.101")
  (exports (byte_sum . %byte_sum.46) (copy_members . %copy_members.47)
    (first_key . %first_key.44) (found_key . %found_key.45)
    (ids . %ids.42) (members . %members.43)
    (members_are . %members_are.40) (row . %row.41)
    (rows . %rows.38) (seed . %seed.39)
    (store_slice . %store_slice.36) (sum_bytes . %sum_bytes.37)
    (tail_id . %tail_id.34) (tail_of_row . %tail_of_row.35)
    (widen_first_id . %widen_first_id.33))
  (contract-types)
  (kernel-declaration (%kernel.68 () (exported #f) (Kernel)))
  (public-ledger-declaration
    (public-ledger-array
      (%members.43
        (0)
        (exported #t)
        (__compact_Cell
          (tvector
            3
            (tstruct Member (id (tunsigned 65535)) (key (tbytes 4))))))
      (%row.41
        (1)
        (exported #t)
        (__compact_Cell
          (ttuple
            (tstruct Member (id (tunsigned 65535)) (key (tbytes 4)))
            (tunsigned 65535))))
      (%ids.42
        (2)
        (exported #t)
        (Map (tunsigned 255) (tunsigned 65535)))
      (%found_key.45
        (3)
        (exported #t)
        (__compact_Cell (tbytes 4)))
      (%tail_id.34
        (4)
        (exported #t)
        (__compact_Cell (tunsigned 65535)))
      (%byte_sum.46
        (5)
        (exported #t)
        (__compact_Cell (tunsigned 4294967295)))
      (%rows.38
        (6)
        (exported #t)
        (Map (tunsigned 255) (tvector 3 (tfield (field-native))))))
    (constructor () (tuple)))
  (export-typedef
    Member
    ()
    (tstruct Member (id (tunsigned 65535)) (key (tbytes 4))))
  (circuit %seed.39 (exported #t) (pure #f) (proof #t)
    ((%ms.64
       (tvector
         3
         (tstruct Member (id (tunsigned 65535)) (key (tbytes 4)))))
      (%m.65
        (tstruct Member (id (tunsigned 65535)) (key (tbytes 4))))
      (%n.66 (tunsigned 65535)))
    (ttuple)
    (seq (public-ledger %members.43 write (0) write (ttuple)
           (instructions
             (push (storage #f) (value (state-value cell (align 0 1))))
             (push
               (storage #t)
               (value (state-value cell (var-ref %ms.64))))
             (ins (cached #f) (n 1)))
           (var-ref %ms.64))
         (let* (((%tmp.67
                   (ttuple
                     (tstruct
                       Member
                       (id (tunsigned 65535))
                       (key (tbytes 4)))
                     (tunsigned 65535))) (tuple
                                           (single (var-ref %m.65))
                                           (single (var-ref %n.66)))))
           (public-ledger %row.41 write (1) write (ttuple)
             (instructions
               (push (storage #f) (value (state-value cell (align 1 1))))
               (push
                 (storage #t)
                 (value (state-value cell (var-ref %tmp.67))))
               (ins (cached #f) (n 1)))
             (var-ref %tmp.67)))
         (return (tuple))))
  (circuit %copy_members.47 (exported #t) (pure #f) (proof #t) ()
    (ttuple)
    (seq (fold
           3
           (circuit
             ((%t.59 (ttuple)) (%i.60 (tunsigned 2)))
             (ttuple)
             (seq (seq (let* (((%tmp.61 (tunsigned 255)) (safe-cast
                                                           (tunsigned 255)
                                                           (tunsigned 2)
                                                           (var-ref
                                                             %i.60))))
                         (let* (((%tmp.62 (tunsigned 65535)) (elt-ref
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
                                                                   %members.43
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
                                                                   %i.60))
                                                               id
                                                               0)))
                           (public-ledger %ids.42 update (2) insert (ttuple)
                             (instructions
                               (idx (cached #f)
                                    (pushPath #t)
                                    (path ((align 2 1))))
                               (push
                                 (storage #f)
                                 (value
                                   (state-value cell (var-ref %tmp.61))))
                               (push
                                 (storage #t)
                                 (value
                                   (state-value
                                     ADT
                                     (var-ref %tmp.62)
                                     (tunsigned 65535))))
                               (ins (cached #f) (n 1))
                               (ins (cached #t) (n 1)))
                             (var-ref %tmp.61) (var-ref %tmp.62))))
                       (tuple))
                  (var-ref %t.59)))
           ((tuple) (ttuple))
           ((tuple (single '0) (single '1) (single '2))
             (ttuple (tunsigned 0) (tunsigned 1) (tunsigned 2))
             (tunsigned 2)))
         (return (tuple))))
  (circuit %first_key.44 (exported #t) (pure #f) (proof #t) ()
    (tbytes 4)
    (let* (((%k.63 (tbytes 4)) (elt-ref
                                 (tuple-ref
                                   (public-ledger %members.43 read (0) read
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
      (seq (public-ledger %found_key.45 write (3) write (ttuple)
             (instructions
               (push (storage #f) (value (state-value cell (align 3 1))))
               (push
                 (storage #t)
                 (value (state-value cell (var-ref %k.63))))
               (ins (cached #f) (n 1)))
             (var-ref %k.63))
           (return (var-ref %k.63)))))
  (circuit %tail_of_row.35 (exported #t) (pure #f) (proof #t) ()
    (tunsigned 65535)
    (let* (((%tail.53 (ttuple (tunsigned 65535))) (tuple-slice
                                                    (ttuple
                                                      (tstruct
                                                        Member
                                                        (id (tunsigned
                                                              65535))
                                                        (key (tbytes 4)))
                                                      (tunsigned 65535))
                                                    (public-ledger %row.41 read (1) read
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
      (seq (let* (((%tmp.54 (tunsigned 65535)) (tuple-ref
                                                 (var-ref %tail.53)
                                                 0)))
             (public-ledger %tail_id.34 write (4) write (ttuple)
               (instructions
                 (push (storage #f) (value (state-value cell (align 4 1))))
                 (push
                   (storage #t)
                   (value (state-value cell (var-ref %tmp.54))))
                 (ins (cached #f) (n 1)))
               (var-ref %tmp.54)))
           (return (tuple-ref (var-ref %tail.53) 0)))))
  (circuit %sum_bytes.37 (exported #t) (pure #f) (proof #t)
    ((%b.57 (tbytes 4))) (tunsigned 4294967295)
    (let* (((%total.58 (tunsigned 4294967295)) (fold
                                                 4
                                                 (circuit
                                                   ((%a.55
                                                      (tunsigned
                                                        4294967295))
                                                     (%x.56
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
                                                              %a.55))
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
                                                                %x.56)))))))
                                                 ((safe-cast
                                                    (tunsigned 4294967295)
                                                    (tunsigned 0)
                                                    '0)
                                                   (tunsigned 4294967295))
                                                 ((var-ref %b.57)
                                                   (tbytes 4)
                                                   (tunsigned 255)))))
      (seq (public-ledger %byte_sum.46 write (5) write (ttuple)
             (instructions
               (push (storage #f) (value (state-value cell (align 5 1))))
               (push
                 (storage #t)
                 (value (state-value cell (var-ref %total.58))))
               (ins (cached #f) (n 1)))
             (var-ref %total.58))
           (return (var-ref %total.58)))))
  (circuit %store_slice.36 (exported #t) (pure #f) (proof #t)
    ((%xs.49 (tvector 6 (tfield (field-native))))) (ttuple)
    (seq (let* (((%mid.51 (tvector 3 (tfield (field-native)))) (tuple-slice
                                                                 (tvector
                                                                   6
                                                                   (tfield
                                                                     (field-native)))
                                                                 (var-ref
                                                                   %xs.49)
                                                                 2
                                                                 3)))
           (let* (((%tmp.50 (tunsigned 255)) (safe-cast
                                               (tunsigned 255)
                                               (tunsigned 0)
                                               '0)))
             (public-ledger %rows.38 update (6) insert (ttuple)
               (instructions (idx (cached #f) (pushPath #t) (path ((align 6 1))))
                 (push
                   (storage #f)
                   (value (state-value cell (var-ref %tmp.50))))
                 (push
                   (storage #t)
                   (value
                     (state-value
                       ADT
                       (var-ref %mid.51)
                       (tvector 3 (tfield (field-native))))))
                 (ins (cached #f) (n 1)) (ins (cached #t) (n 1)))
               (var-ref %tmp.50) (var-ref %mid.51))))
         (return (tuple))))
  (circuit %widen_first_id.33 (exported #t) (pure #f) (proof #t) ()
    (ttuple)
    (seq (let* (((%tmp.52 (tunsigned 4294967295)) (safe-cast
                                                    (tunsigned 4294967295)
                                                    (tunsigned 65535)
                                                    (elt-ref
                                                      (tuple-ref
                                                        (public-ledger %members.43 read
                                                          (0) read
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
                                                              (cached #f)
                                                              (result
                                                                (void)))))
                                                        0)
                                                      id
                                                      0))))
           (public-ledger %byte_sum.46 write (5) write (ttuple)
             (instructions
               (push (storage #f) (value (state-value cell (align 5 1))))
               (push
                 (storage #t)
                 (value (state-value cell (var-ref %tmp.52))))
               (ins (cached #f) (n 1)))
             (var-ref %tmp.52)))
         (return (tuple))))
  (circuit %members_are.40 (exported #t) (pure #f) (proof #t)
    ((%ms.48
       (tvector
         3
         (tstruct Member (id (tunsigned 65535)) (key (tbytes 4))))))
    (tboolean)
    (return
      (== (tvector
            3
            (tstruct Member (id (tunsigned 65535)) (key (tbytes 4))))
          (public-ledger %members.43 read (0) read
            (tvector
              3
              (tstruct Member (id (tunsigned 65535)) (key (tbytes 4))))
            (instructions
              (dup (n 0))
              (idx (cached #f) (pushPath #f) (path ((align 0 1))))
              (popeq (cached #f) (result (void)))))
          (var-ref %ms.48)))))
