(analyzed-ir (compiler-version "0.33.122") (language-version "0.25.107")
  (runtime-version "0.18.107")
  (exports (clear . %clear.3) (get . %get.4)
    (public_key . %public_key.1) (set . %set.2)
    (value . %value.0))
  (contract-types)
  (kernel-declaration (%kernel.22 () (exported #f) (Kernel)))
  (public-ledger-declaration
    (public-ledger-array
      (%authority.9
        (0)
        (exported #f)
        (__compact_Cell (tbytes 32)))
      (%value.0
        (1)
        (exported #t)
        (__compact_Cell (tfield (field-native))))
      (%state.8
        (2)
        (exported #f)
        (__compact_Cell (tenum STATE unset set))))
    (constructor
      ((%v.23 (tfield (field-native))))
      (seq (let* (((%sk.24 (tbytes 32)) (call
                                          %private$secret_key.6)))
             (seq (let* (((%tmp.25 (tbytes 32)) (call
                                                  %public_key.1
                                                  (var-ref %sk.24))))
                    (public-ledger %authority.9 (0) write (ttuple)
                      (instructions
                        (push
                          (storage #f)
                          (value (state-value cell (align 0 1))))
                        (push
                          (storage #t)
                          (value (state-value cell (var-ref %tmp.25))))
                        (ins (cached #f) (n 1)))
                      (var-ref %tmp.25)))
                  (public-ledger %value.0 (1) write (ttuple)
                    (instructions
                      (push
                        (storage #f)
                        (value (state-value cell (align 1 1))))
                      (push
                        (storage #t)
                        (value (state-value cell (var-ref %v.23))))
                      (ins (cached #f) (n 1)))
                    (var-ref %v.23))
                  (public-ledger %state.8 (2) write (ttuple)
                    (instructions
                      (push
                        (storage #f)
                        (value (state-value cell (align 2 1))))
                      (push
                        (storage #t)
                        (value
                          (state-value
                            cell
                            (enum-ref (tenum STATE unset set) set))))
                      (ins (cached #f) (n 1)))
                    (enum-ref (tenum STATE unset set) set))))
           (return (tuple)))))
  (export-typedef
    Maybe
    (T)
    (tstruct Maybe (is_some (tboolean)) (value T)))
  (circuit %some.17 (exported #f) (pure #t) (proof #f)
    ((%value.21 (tfield (field-native))))
    (tstruct
      Maybe
      (is_some (tboolean))
      (value (tfield (field-native))))
    (return
      (new (tstruct
             Maybe
             (is_some (tboolean))
             (value (tfield (field-native))))
           '#t
           (var-ref %value.21))))
  (circuit %none.18 (exported #f) (pure #t) (proof #f) ()
    (tstruct
      Maybe
      (is_some (tboolean))
      (value (tfield (field-native))))
    (return
      (new (tstruct
             Maybe
             (is_some (tboolean))
             (value (tfield (field-native))))
           '#f
           (default (tfield (field-native))))))
  (native
    %persistentHash.13
    (entry "__compactRuntime.persistentHash" circuit)
    ((%value.20 (tvector 2 (tbytes 32))))
    (tbytes 32))
  (witness %private$secret_key.6 () (tbytes 32))
  (circuit %in_state.11 (exported #f) (pure #f) (proof #f)
    ((%s.19 (tenum STATE unset set))) (tboolean)
    (return
      (== (tenum STATE unset set)
          (public-ledger %state.8 (2) read (tenum STATE unset set)
            (instructions
              (dup (n 0))
              (idx (cached #f) (pushPath #f) (path ((align 2 1))))
              (popeq (cached #f) (result (void)))))
          (var-ref %s.19))))
  (circuit %set.2 (exported #t) (pure #f) (proof #t)
    ((%v.14 (tfield (field-native)))) (ttuple)
    (seq (seq (assert
                (call
                  %in_state.11
                  (enum-ref (tenum STATE unset set) unset))
                "set: attempted to overwrite recorded value")
              (let* (((%sk.15 (tbytes 32)) (call %private$secret_key.6)))
                (let* (((%apk.16 (tbytes 32)) (call
                                                %public_key.1
                                                (var-ref %sk.15))))
                  (seq (public-ledger %authority.9 (0) write (ttuple)
                         (instructions
                           (push
                             (storage #f)
                             (value (state-value cell (align 0 1))))
                           (push
                             (storage #t)
                             (value (state-value cell (var-ref %apk.16))))
                           (ins (cached #f) (n 1)))
                         (var-ref %apk.16))
                       (public-ledger %value.0 (1) write (ttuple)
                         (instructions
                           (push
                             (storage #f)
                             (value (state-value cell (align 1 1))))
                           (push
                             (storage #t)
                             (value (state-value cell (var-ref %v.14))))
                           (ins (cached #f) (n 1)))
                         (var-ref %v.14))
                       (public-ledger %state.8 (2) write (ttuple)
                         (instructions
                           (push
                             (storage #f)
                             (value (state-value cell (align 2 1))))
                           (push
                             (storage #t)
                             (value
                               (state-value
                                 cell
                                 (enum-ref (tenum STATE unset set) set))))
                           (ins (cached #f) (n 1)))
                         (enum-ref (tenum STATE unset set) set))))))
         (return (tuple))))
  (circuit %get.4 (exported #t) (pure #f) (proof #t) ()
    (tstruct
      Maybe
      (is_some (tboolean))
      (value (tfield (field-native))))
    (return
      (if (call
            %in_state.11
            (enum-ref (tenum STATE unset set) set))
          (call
            %some.17
            (public-ledger %value.0 (1) read (tfield (field-native))
              (instructions
                (dup (n 0))
                (idx (cached #f) (pushPath #f) (path ((align 1 1))))
                (popeq (cached #f) (result (void))))))
          (call %none.18))))
  (circuit %clear.3 (exported #t) (pure #f) (proof #t) () (ttuple)
    (seq (seq (assert
                (call %in_state.11 (enum-ref (tenum STATE unset set) set))
                "clear: no value is currently recorded")
              (let* (((%sk.5 (tbytes 32)) (call %private$secret_key.6)))
                (let* (((%apk.7 (tbytes 32)) (call
                                               %public_key.1
                                               (var-ref %sk.5))))
                  (seq (assert
                         (== (tbytes 32)
                             (var-ref %apk.7)
                             (public-ledger %authority.9 (0) read (tbytes 32)
                               (instructions
                                 (dup (n 0))
                                 (idx (cached #f)
                                      (pushPath #f)
                                      (path ((align 0 1))))
                                 (popeq (cached #f) (result (void))))))
                         "clear: attempted clear without proper authorization")
                       (let* (((%tmp.10 (tbytes 32)) (default
                                                       (tbytes 32))))
                         (public-ledger %authority.9 (0) write (ttuple)
                           (instructions
                             (push
                               (storage #f)
                               (value (state-value cell (align 0 1))))
                             (push
                               (storage #t)
                               (value
                                 (state-value cell (var-ref %tmp.10))))
                             (ins (cached #f) (n 1)))
                           (var-ref %tmp.10)))
                       (public-ledger %value.0 (1) write (ttuple)
                         (instructions
                           (push
                             (storage #f)
                             (value (state-value cell (align 1 1))))
                           (push
                             (storage #t)
                             (value
                               (state-value
                                 cell
                                 (default (tfield (field-native))))))
                           (ins (cached #f) (n 1)))
                         (default (tfield (field-native))))
                       (public-ledger %state.8 (2) write (ttuple)
                         (instructions
                           (push
                             (storage #f)
                             (value (state-value cell (align 2 1))))
                           (push
                             (storage #t)
                             (value
                               (state-value
                                 cell
                                 (enum-ref
                                   (tenum STATE unset set)
                                   unset))))
                           (ins (cached #f) (n 1)))
                         (enum-ref (tenum STATE unset set) unset))))))
         (return (tuple))))
  (circuit %public_key.1 (exported #t) (pure #t) (proof #f)
    ((%sk.12 (tbytes 32))) (tbytes 32)
    (return
      (call
        %persistentHash.13
        (tuple
          (single
            '#vu8(108 97 114 101 115 58 116 105 110 121 58 112 107 58 0
                  0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0))
          (single (var-ref %sk.12)))))))
