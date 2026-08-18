(analyzed-ir (compiler-version "0.33.122")
 (language-version "0.25.107") (runtime-version "0.18.107")
 (exports
   (spend . %spend.0)
   (zerocash_mint . %zerocash_mint.1))
 (contract-types)
 (kernel-declaration (%kernel.56 () (exported #f) (Kernel)))
 (public-ledger-declaration
   (public-ledger-array
     (%nullifiers.31
       (0)
       (exported #f)
       (Set (tstruct nullifier (bytes (tbytes 32)))))
     (%commitments.28
       (1)
       (exported #f)
       (HistoricMerkleTree
         32
         (tstruct commitment (bytes (tbytes 32)))))
     (%ciphertexts.27
       (2)
       (exported #f)
       (__compact_Cell (topaque "Uint8Array"))))
   (constructor () (tuple)))
 (circuit %merkleTreePathRoot.30 (exported #f) (pure #t) (proof #f)
   ((%path.50
      (tstruct
        MerkleTreePath
        (leaf (tstruct commitment (bytes (tbytes 32))))
        (path
          (tvector
            32
            (tstruct
              MerkleTreePathEntry
              (sibling
                (tstruct MerkleTreeDigest (field (tfield (field-native)))))
              (goes_left (tboolean))))))))
   (tstruct MerkleTreeDigest (field (tfield (field-native))))
   (return
     (new (tstruct
            MerkleTreeDigest
            (field (tfield (field-native))))
          (fold
            32
            (fref %merkleTreePathEntryRoot.51)
            ((call
               %degradeToTransient.42
               (call
                 %persistentHash.45
                 (new (tstruct
                        LeafPreimage
                        (domain_sep (tbytes 6))
                        (data (tstruct commitment (bytes (tbytes 32)))))
                      '#vu8(109 100 110 58 108 104)
                      (elt-ref (var-ref %path.50) leaf 0))))
              (tfield (field-native)))
            ((elt-ref (var-ref %path.50) path 1)
              (tvector
                32
                (tstruct
                  MerkleTreePathEntry
                  (sibling
                    (tstruct
                      MerkleTreeDigest
                      (field (tfield (field-native)))))
                  (goes_left (tboolean))))
              (tstruct
                MerkleTreePathEntry
                (sibling
                  (tstruct
                    MerkleTreeDigest
                    (field (tfield (field-native)))))
                (goes_left (tboolean))))))))
 (circuit %merkleTreePathEntryRoot.51 (exported #f) (pure #t)
   (proof #f)
   ((%recursiveDigest.52 (tfield (field-native)))
     (%entry.53
       (tstruct
         MerkleTreePathEntry
         (sibling
           (tstruct MerkleTreeDigest (field (tfield (field-native)))))
         (goes_left (tboolean)))))
   (tfield (field-native))
   (let* (((%left.54 (tfield (field-native))) (if (elt-ref
                                                    (var-ref %entry.53)
                                                    goes_left
                                                    1)
                                                  (var-ref
                                                    %recursiveDigest.52)
                                                  (elt-ref
                                                    (elt-ref
                                                      (var-ref %entry.53)
                                                      sibling
                                                      0)
                                                    field
                                                    0))))
     (let* (((%right.55 (tfield (field-native))) (if (elt-ref
                                                       (var-ref %entry.53)
                                                       goes_left
                                                       1)
                                                     (elt-ref
                                                       (elt-ref
                                                         (var-ref
                                                           %entry.53)
                                                         sibling
                                                         0)
                                                       field
                                                       0)
                                                     (var-ref
                                                       %recursiveDigest.52))))
       (return
         (call
           %transientHash.47
           (tuple
             (single (var-ref %left.54))
             (single (var-ref %right.55))))))))
 (native %transientHash.47
   (entry "__compactRuntime.transientHash" circuit)
   (type-arguments (tvector 2 (tfield (field-native))))
   ((%value.48 (tvector 2 (tfield (field-native)))))
   (tfield (field-native)))
 (native %persistentHash.11
   (entry "__compactRuntime.persistentHash" circuit)
   (type-arguments (tbytes 32)) ((%value.49 (tbytes 32)))
   (tbytes 32))
 (native %persistentHash.5
   (entry "__compactRuntime.persistentHash" circuit)
   (type-arguments (tvector 4 (tbytes 32)))
   ((%value.44 (tvector 4 (tbytes 32)))) (tbytes 32))
 (native %persistentHash.45
   (entry "__compactRuntime.persistentHash" circuit)
   (type-arguments
     (tstruct
       LeafPreimage
       (domain_sep (tbytes 6))
       (data (tstruct commitment (bytes (tbytes 32))))))
   ((%value.46
      (tstruct
        LeafPreimage
        (domain_sep (tbytes 6))
        (data (tstruct commitment (bytes (tbytes 32)))))))
   (tbytes 32))
 (native %degradeToTransient.42
   (entry "__compactRuntime.degradeToTransient" circuit)
   (type-arguments) ((%x.43 (tbytes 32)))
   (tfield (field-native)))
 (witness
   %private$zk_secret_key.15
   ()
   (tstruct zk_secret_key (bytes (tbytes 32))))
 (witness
   %private$remove_coin.26
   ((%coin.41
      (tstruct
        coin_info
        (nonce (tstruct Nonce (bytes (tbytes 32))))
        (opening (tstruct opening (bytes (tbytes 32)))))))
   (ttuple))
 (witness
   %private$zk_public_key.34
   ()
   (tstruct zk_public_key (bytes (tbytes 32))))
 (witness
   %private$add_coin.36
   ((%coin.39
      (tstruct
        coin_info
        (nonce (tstruct Nonce (bytes (tbytes 32))))
        (opening (tstruct opening (bytes (tbytes 32)))))))
   (ttuple))
 (witness
   %context$path_of.20
   ((%cm.40 (tstruct commitment (bytes (tbytes 32)))))
   (tstruct
     MerkleTreePath
     (leaf (tstruct commitment (bytes (tbytes 32))))
     (path
       (tvector
         32
         (tstruct
           MerkleTreePathEntry
           (sibling
             (tstruct MerkleTreeDigest (field (tfield (field-native)))))
           (goes_left (tboolean)))))))
 (witness
   %context$new_coin_info.22
   ()
   (tstruct
     coin_info
     (nonce (tstruct Nonce (bytes (tbytes 32))))
     (opening (tstruct opening (bytes (tbytes 32))))))
 (witness
   %context$encrypt.25
   ((%pk.37 (topaque "Uint8Array"))
     (%coin.38
       (tstruct
         coin_info
         (nonce (tstruct Nonce (bytes (tbytes 32))))
         (opening (tstruct opening (bytes (tbytes 32)))))))
   (topaque "Uint8Array"))
 (circuit %spend.0 (exported #t) (pure #f) (proof #t)
   ((%dest_public_key.12
      (tstruct
        public_key
        (zk (tstruct zk_public_key (bytes (tbytes 32))))
        (encryption (topaque "Uint8Array"))))
     (%input_coin.13
       (tstruct
         coin_info
         (nonce (tstruct Nonce (bytes (tbytes 32))))
         (opening (tstruct opening (bytes (tbytes 32)))))))
   (ttuple)
   (seq (let* (((%source_secret_key.14
                  (tstruct zk_secret_key (bytes (tbytes 32)))) (call
                                                                 %private$zk_secret_key.15)))
          (let* (((%old_nullifier.16
                    (tstruct nullifier (bytes (tbytes 32)))) (call
                                                               %derive_nullifier.6
                                                               (var-ref
                                                                 %input_coin.13)
                                                               (var-ref
                                                                 %source_secret_key.14))))
            (seq (assert
                   (if (public-ledger %nullifiers.31 read (0) member (tboolean)
                         (instructions (dup (n 0))
                           (idx (cached #f)
                                (pushPath #f)
                                (path ((align 0 1))))
                           (push
                             (storage #f)
                             (value
                               (state-value
                                 cell
                                 (var-ref %old_nullifier.16))))
                           (member) (popeq (cached #t) (result (void))))
                         (var-ref %old_nullifier.16))
                       '#f
                       '#t)
                   "spend: Coin already spent")
                 (public-ledger %nullifiers.31 update (0) insert (ttuple)
                   (instructions (idx (cached #f) (pushPath #t) (path ((align 0 1))))
                     (push
                       (storage #f)
                       (value
                         (state-value cell (var-ref %old_nullifier.16))))
                     (push (storage #t) (value (state-value null)))
                     (ins (cached #f) (n 1)) (ins (cached #t) (n 1)))
                   (var-ref %old_nullifier.16))
                 (let* (((%source_public_key.17
                           (tstruct zk_public_key (bytes (tbytes 32)))) (call
                                                                          %derive_zk_public_key.9
                                                                          (var-ref
                                                                            %source_secret_key.14))))
                   (let* (((%old_commitment.18
                             (tstruct commitment (bytes (tbytes 32)))) (call
                                                                         %commitment_from_coin_info.2
                                                                         (var-ref
                                                                           %input_coin.13)
                                                                         (var-ref
                                                                           %source_public_key.17))))
                     (let* (((%commitment_path.19
                               (tstruct
                                 MerkleTreePath
                                 (leaf
                                   (tstruct
                                     commitment
                                     (bytes (tbytes 32))))
                                 (path
                                   (tvector
                                     32
                                     (tstruct
                                       MerkleTreePathEntry
                                       (sibling
                                         (tstruct
                                           MerkleTreeDigest
                                           (field
                                             (tfield (field-native)))))
                                       (goes_left (tboolean))))))) (call
                                                                     %context$path_of.20
                                                                     (var-ref
                                                                       %old_commitment.18))))
                       (seq (assert
                              (if (let* (((%tmp.29
                                            (tstruct
                                              MerkleTreeDigest
                                              (field
                                                (tfield (field-native))))) (call
                                                                             %merkleTreePathRoot.30
                                                                             (var-ref
                                                                               %commitment_path.19))))
                                    (public-ledger %commitments.28 read (1) checkRoot
                                      (tboolean)
                                      (instructions (dup (n 0))
                                        (idx (cached #f)
                                             (pushPath #f)
                                             (path ((align 1 1))))
                                        (idx (cached #f)
                                             (pushPath #f)
                                             (path ((align 2 1))))
                                        (push
                                          (storage #f)
                                          (value
                                            (state-value
                                              cell
                                              (var-ref %tmp.29))))
                                        (member)
                                        (popeq
                                          (cached #t)
                                          (result (void))))
                                      (var-ref %tmp.29)))
                                  (== (tstruct
                                        commitment
                                        (bytes (tbytes 32)))
                                      (var-ref %old_commitment.18)
                                      (elt-ref
                                        (var-ref %commitment_path.19)
                                        leaf
                                        0))
                                  '#f)
                              "spend: Illegal state: merkle path not recognized by public state")
                            (let* (((%fresh_coin_info.21
                                      (tstruct
                                        coin_info
                                        (nonce
                                          (tstruct
                                            Nonce
                                            (bytes (tbytes 32))))
                                        (opening
                                          (tstruct
                                            opening
                                            (bytes (tbytes 32)))))) (call
                                                                      %context$new_coin_info.22)))
                              (let* (((%fresh_commitment.23
                                        (tstruct
                                          commitment
                                          (bytes (tbytes 32)))) (call
                                                                  %commitment_from_coin_info.2
                                                                  (var-ref
                                                                    %fresh_coin_info.21)
                                                                  (elt-ref
                                                                    (var-ref
                                                                      %dest_public_key.12)
                                                                    zk
                                                                    0))))
                                (seq (public-ledger %commitments.28 update (1) insert
                                       (ttuple)
                                       (instructions
                                         (idx (cached #f)
                                              (pushPath #t)
                                              (path ((align 1 1))))
                                         (idx (cached #f)
                                              (pushPath #t)
                                              (path ((align 0 1))))
                                         (dup (n 2))
                                         (idx (cached #f)
                                              (pushPath #f)
                                              (path ((align 1 1))))
                                         (push
                                           (storage #t)
                                           (value
                                             (state-value
                                               cell
                                               (leaf-hash
                                                 (var-ref
                                                   %fresh_commitment.23)))))
                                         (ins (cached #f) (n 1))
                                         (ins (cached #t) (n 1))
                                         (idx (cached #f)
                                              (pushPath #t)
                                              (path ((align 1 1))))
                                         (addi (immediate 1))
                                         (ins (cached #t) (n 1))
                                         (idx (cached #f)
                                              (pushPath #t)
                                              (path ((align 2 1))))
                                         (dup (n 2))
                                         (idx (cached #f)
                                              (pushPath #f)
                                              (path ((align 0 1))))
                                         (root)
                                         (push
                                           (storage #t)
                                           (value (state-value null)))
                                         (ins (cached #f) (n 1))
                                         (ins (cached #t) (n 2)))
                                       (var-ref %fresh_commitment.23))
                                     (let* (((%ciphertext.24
                                               (topaque "Uint8Array")) (call
                                                                         %context$encrypt.25
                                                                         (elt-ref
                                                                           (var-ref
                                                                             %dest_public_key.12)
                                                                           encryption
                                                                           1)
                                                                         (var-ref
                                                                           %fresh_coin_info.21))))
                                       (seq (public-ledger %ciphertexts.27 write (2)
                                              write (ttuple)
                                              (instructions
                                                (push
                                                  (storage #f)
                                                  (value
                                                    (state-value
                                                      cell
                                                      (align 2 1))))
                                                (push
                                                  (storage #t)
                                                  (value
                                                    (state-value
                                                      cell
                                                      (var-ref
                                                        %ciphertext.24))))
                                                (ins (cached #f) (n 1)))
                                              (var-ref %ciphertext.24))
                                            (call
                                              %private$remove_coin.26
                                              (var-ref
                                                %input_coin.13))))))))))))))
        (return (tuple))))
 (circuit %zerocash_mint.1 (exported #t) (pure #f) (proof #t) ()
   (ttuple)
   (seq (let* (((%coin.32
                  (tstruct
                    coin_info
                    (nonce (tstruct Nonce (bytes (tbytes 32))))
                    (opening (tstruct opening (bytes (tbytes 32)))))) (call
                                                                        %context$new_coin_info.22)))
          (let* (((%pk.33 (tstruct zk_public_key (bytes (tbytes 32)))) (call
                                                                         %private$zk_public_key.34)))
            (seq (call %private$add_coin.36 (var-ref %coin.32))
                 (let* (((%cm.35 (tstruct commitment (bytes (tbytes 32)))) (call
                                                                             %commitment_from_coin_info.2
                                                                             (var-ref
                                                                               %coin.32)
                                                                             (var-ref
                                                                               %pk.33))))
                   (public-ledger %commitments.28 update (1) insert (ttuple)
                     (instructions (idx (cached #f) (pushPath #t) (path ((align 1 1))))
                       (idx (cached #f) (pushPath #t) (path ((align 0 1))))
                       (dup (n 2))
                       (idx (cached #f) (pushPath #f) (path ((align 1 1))))
                       (push
                         (storage #t)
                         (value
                           (state-value
                             cell
                             (leaf-hash (var-ref %cm.35)))))
                       (ins (cached #f) (n 1)) (ins (cached #t) (n 1))
                       (idx (cached #f) (pushPath #t) (path ((align 1 1))))
                       (addi (immediate 1)) (ins (cached #t) (n 1))
                       (idx (cached #f) (pushPath #t) (path ((align 2 1))))
                       (dup (n 2))
                       (idx (cached #f) (pushPath #f) (path ((align 0 1))))
                       (root)
                       (push (storage #t) (value (state-value null)))
                       (ins (cached #f) (n 1)) (ins (cached #t) (n 2)))
                     (var-ref %cm.35))))))
        (return (tuple))))
 (circuit %derive_nullifier.6 (exported #f) (pure #t) (proof #f)
   ((%coin.7
      (tstruct
        coin_info
        (nonce (tstruct Nonce (bytes (tbytes 32))))
        (opening (tstruct opening (bytes (tbytes 32))))))
     (%sk.8 (tstruct zk_secret_key (bytes (tbytes 32)))))
   (tstruct nullifier (bytes (tbytes 32)))
   (return
     (new (tstruct nullifier (bytes (tbytes 32)))
          (call
            %persistentHash.5
            (tuple
              (single
                '#vu8(108 97 114 101 115 58 122 101 114 111 99 97 115 104
                      58 99 111 109 109 105 116 0 0 0 0 0 0 0 0 0 0 0))
              (single
                (elt-ref (elt-ref (var-ref %coin.7) nonce 0) bytes 0))
              (single
                (elt-ref (elt-ref (var-ref %coin.7) opening 1) bytes 0))
              (single (elt-ref (var-ref %sk.8) bytes 0)))))))
 (circuit %derive_zk_public_key.9 (exported #f) (pure #t) (proof #f)
   ((%sk.10 (tstruct zk_secret_key (bytes (tbytes 32)))))
   (tstruct zk_public_key (bytes (tbytes 32)))
   (return
     (new (tstruct zk_public_key (bytes (tbytes 32)))
          (call
            %persistentHash.11
            (elt-ref (var-ref %sk.10) bytes 0)))))
 (circuit %commitment_from_coin_info.2 (exported #f) (pure #t)
   (proof #f)
   ((%coin.3
      (tstruct
        coin_info
        (nonce (tstruct Nonce (bytes (tbytes 32))))
        (opening (tstruct opening (bytes (tbytes 32))))))
     (%pk.4 (tstruct zk_public_key (bytes (tbytes 32)))))
   (tstruct commitment (bytes (tbytes 32)))
   (return
     (new (tstruct commitment (bytes (tbytes 32)))
          (call
            %persistentHash.5
            (tuple
              (single
                '#vu8(108 97 114 101 115 58 122 101 114 111 99 97 115 104
                      58 99 111 109 109 105 116 0 0 0 0 0 0 0 0 0 0 0))
              (single
                (elt-ref (elt-ref (var-ref %coin.3) nonce 0) bytes 0))
              (single
                (elt-ref (elt-ref (var-ref %coin.3) opening 1) bytes 0))
              (single (elt-ref (var-ref %pk.4) bytes 0))))))))
