This page is a work in progress and represents my (very limited) understanding of M-Labs hardware and software.
Any and all corrections, additions, or improvements are more than welcome.



Getting started with Sinara hardware
====================================

.. _Sinara hardware:

Sinara hardware
-----------------------------
Sinara 
^^^^^^^^^^^^^^

Kasli
-----
The `Kasli <https://github.com/sinara-hw/Kasli/wiki>`_ board is an FPGA carrier which can control other Sinara boards.
Schematics for the Kasli board are available `here <https://github.com/sinara-hw/Kasli/releases>`_.

Hardware setup
^^^^^^^^^^^^^^
The process of setting up a Kasli is described in the "Installing ARTIQ" chapter of the ARTIQ manual under the section "Preparing
the core device FPGA board".

How to use Kasli with ARTIQ
^^^^^^^^^^^^^^^^^^^^^^^^^^^

DIO (SMA/BNC)
-------------
DIO (Digital In/Out) boards come in three varieties, depending on the connector type:
`BNC <https://github.com/sinara-hw/DIO_BNC/wiki>`_, `SMA <https://github.com/sinara-hw/DIO_SMA/wiki>`_,
and `RJ45 <https://github.com/sinara-hw/DIO_RJ45/wiki>`_.  BNC and SMA boards are functionally equivalent
[as far as I can see] and operate at input/output voltages of about 2 V.  RJ45 boards operate through
low-voltage differential signaling (LVDS). Schematics are available for 
`BNC <https://github.com/sinara-hw/DIO_BNC/releases>`_, `SMA <https://github.com/sinara-hw/DIO_SMA/releases>`_,
and `RJ45 <https://github.com/sinara-hw/DIO_RJ45/releases>`_.

Hardware setup
^^^^^^^^^^^^^^
BNC and SMA boards have only one EEM port and do not require clocking.  These boards each have two banks of four
TTL digital input/outputs.  Whether a bank can input or output can be set using the switches on the two-switch DIP
(marked SW9 on the boards).

How to use DIO boards with ARTIQ
^^^^^^^^^^^^^^^^^^^^^^^^^^^
Coding tutorials on using TTL inputs/outputs are offered in the ARTIQ manual.

Input
`````
Input TTLs are read digitally (i.e. as having a low incoming signal or a high incoming signal).  This feature can
be used to count the number of discrete inputs (called events) received by an input channel over time.

The process of counting events involves two steps:

 * Defining a certain input type (a rising edge, a falling edge, or both) as an event for a certain amount of time
 * Counting the number of events stored in the FIFO buffer by the time this period ends

[provide code examples for counting inputs.  Note that the FIFO buffer where events are stored can only hold
around 100 events]

Output
``````
Output TTLs can be used to generate digital signals, and their usage is fairly simple.  They can be set low using
the off() method and high using the on() method.  They can also be set high for a given amount of time using the
pulse() method.

[provide code examples for generating pulse trains of different frequencies]

Urukul
------
The `Urukul <https://github.com/sinara-hw/Urukul/wiki>`_ board is a 4 channel DDS frequency synthesizer.  Urukul can
generate RF signals of up to 400 MHz.  Schematics for Urukul are available `here <https://github.com/sinara-hw/Urukul/releases>`_.

Hardware setup
^^^^^^^^^^^^^^

How to use Urukul with ARTIQ
^^^^^^^^^^^^^^^^^^^^^^^^^^^

Sampler
------
Hardware setup
^^^^^^^^^^^^^^

How to use Sampler with ARTIQ
^^^^^^^^^^^^^^^^^^^^^^^^^^^

Zotino
------
Hardware setup
^^^^^^^^^^^^^^

How to use Zotino with ARTIQ
^^^^^^^^^^^^^^^^^^^^^^^^^^^

Novogorny
--------
Hardware setup
^^^^^^^^^^^^^^

How to use Novogorny with ARTIQ
^^^^^^^^^^^^^^^^^^^^^^^^^^^

Grabber
-------
Hardware setup
^^^^^^^^^^^^^^

How to use Grabber with ARTIQ
^^^^^^^^^^^^^^^^^^^^^^^^^^^

SUServo
-------
Hardware setup
^^^^^^^^^^^^^^

How to use SUServo with ARTIQ
^^^^^^^^^^^^^^^^^^^^^^^^^^^
